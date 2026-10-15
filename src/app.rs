use std::{path::PathBuf, sync::mpsc::{self, Receiver, Sender}, thread, time::{Duration, Instant}};

use anyhow::Result;
use reqwest::blocking::Client;
use tiny_skia::Pixmap;

use crate::{alert::PriceAlert, api, drawing::{CHART_HEIGHT, CHART_WIDTH, LEFT, Renderer, Scene, TOP}, log_error,
	model::{Point, Quote, Session, merge_points, time_text}, platform::Native, storage::{self, Store}};

enum Event {
	Quote(Result<Quote, String>),
	History(Session, Result<Vec<Point>, String>),
}

pub struct App {
	pub native: Option<Native>,
	pub drag_offset: Option<(i32, i32)>,
	pub closing: bool,
	renderer: Renderer,
	store: Store,
	client: Client,
	sender: Sender<Event>,
	receiver: Receiver<Event>,
	session: Session,
	points: Vec<Point>,
	quote: Option<Quote>,
	quote_error: Option<String>,
	history_error: Option<String>,
	history_loaded: bool,
	quote_pending: bool,
	history_pending: bool,
	last_quote: Instant,
	last_history: Instant,
	last_save: Instant,
	dirty: bool,
	redraw: bool,
	hover: Option<usize>,
	alerts: Vec<PriceAlert>,
	alert_dirty: bool,
}

impl App {
	pub fn new() -> Result<Self> {
		let directory: PathBuf = storage::data_directory()?;
		let store: Store = Store::open(&directory)?;
		let session: Session = Session::current();
		let points: Vec<Point> = store.load(session)?;
		let alerts: Vec<PriceAlert> = store.load_alerts()?;
		let (sender, receiver): (Sender<Event>, Receiver<Event>) = mpsc::channel();
		let now: Instant = Instant::now();
		Ok(Self {
			native: None, drag_offset: None, closing: false, renderer: Renderer::new()?, store,
			client: api::client()?, sender, receiver, session, points, quote: None,
			quote_error: None, history_error: None, history_loaded: false,
			quote_pending: false, history_pending: false,
			last_quote: now - Duration::from_secs(1), last_history: now - Duration::from_secs(30),
			last_save: now, dirty: false, redraw: true, hover: None, alerts, alert_dirty: false,
		})
	}

	pub fn tick(&mut self) {
		if self.closing {
			return;
		}
		let session: Session = Session::current();
		if session != self.session {
			self.session = session;
			self.points.clear();
			self.quote = None;
			self.hover = None;
			self.history_loaded = false;
			self.history_error = None;
			self.last_history = Instant::now() - Duration::from_secs(30);
			self.dirty = true;
			self.redraw = true;
			self.save();
		}
		while let Ok(event) = self.receiver.try_recv() {
			match event {
				Event::Quote(result) => {
					self.quote_pending = false;
					match result {
						Ok(quote) => {
							self.quote_error = None;
							self.add_quote(quote);
						}
						Err(message) => {
							if self.quote_error.as_ref() != Some(&message) { log_error(&message); }
							self.quote_error = Some(message);
						}
					}
				}
				Event::History(request_session, result) => {
					self.history_pending = false;
					if request_session != self.session { continue; }
					match result {
						Ok(history) => {
							self.history_loaded = !history.is_empty();
							self.history_error = None;
							self.points = merge_points(self.session, &history, &self.points);
							self.dirty = true;
							self.save();
						}
						Err(message) => {
							if self.history_error.as_ref() != Some(&message) { log_error(&message); }
							self.history_error = Some(message);
						}
					}
				}
			}
			self.redraw = true;
		}
		if !self.quote_pending && self.last_quote.elapsed() >= Duration::from_secs(1) {
			self.quote_pending = true;
			self.last_quote = Instant::now();
			let client: Client = self.client.clone();
			let sender: Sender<Event> = self.sender.clone();
			thread::spawn(move || {
				let result: Result<Quote, String> = api::latest(&client).map_err(|error: anyhow::Error| format!("报价请求失败：{error:#}"));
				let _: Result<(), mpsc::SendError<Event>> = sender.send(Event::Quote(result));
			});
		}
		if !self.history_loaded && !self.history_pending && self.last_history.elapsed() >= Duration::from_secs(30) {
			self.history_pending = true;
			self.last_history = Instant::now();
			let client: Client = self.client.clone();
			let sender: Sender<Event> = self.sender.clone();
			let session: Session = self.session;
			thread::spawn(move || {
				let result: Result<Vec<Point>, String> = api::history(&client, session).map_err(|error: anyhow::Error| format!("历史请求失败：{error:#}"));
				let _: Result<(), mpsc::SendError<Event>> = sender.send(Event::History(session, result));
			});
		}
		if (self.dirty || self.alert_dirty) && self.last_save.elapsed() >= Duration::from_secs(30) { self.save(); }
		self.update_hover();
		if self.redraw && self.native.as_ref().is_some_and(|native: &Native| native.visible) {
			if let Err(error) = self.draw() { log_error(&format!("绘制失败：{error:#}")); }
		}
	}

	fn add_quote(&mut self, quote: Quote) {
		let point: Point = quote.point;
		self.check_alert(point);
		if self.session.contains(point) {
			match self.points.last_mut() {
				Some(last) if last.timestamp == point.timestamp => { *last = point; self.dirty = true; }
				Some(last) if last.timestamp > point.timestamp => {}
				_ => { self.points.push(point); self.dirty = true; }
			}
		}
		if let Some(native) = self.native.as_mut() {
			native.set_tooltip(&format!("黄金 {:.2}\n报价时间 {}", point.price, time_text(point.timestamp)));
		}
		self.quote = Some(quote);
	}

	fn check_alert(&mut self, point: Point) {
		if self.alerts.is_empty() { return; }
		let mut next: Vec<PriceAlert> = self.alerts.clone();
		let mut hits: Vec<usize> = Vec::new();
		next.iter_mut().enumerate().for_each(|(index, alert): (usize, &mut PriceAlert)| {
			if alert.observe(point) { alert.remaining -= 1; hits.push(index); }
		});
		if hits.is_empty() {
			self.alerts = next;
			self.alert_dirty = true;
			return;
		}
		// 先持久化次数，重启不会重新消耗已提交给 Windows 的通知。
		if let Err(error) = self.store.save_alerts(&next) {
			log_error(&format!("保存通知次数失败，暂不发送：{error:#}"));
			return;
		}
		// 同一报价命中多个任务时合并通知，避免 Windows 覆盖连续弹出的横幅。
		let details: String = hits.iter().take(4).map(|index: &usize| {
			let alert: &PriceAlert = &next[*index];
			format!("#{} {} {}，剩余{}次", alert.id, alert.direction.label(), alert.target_text(), alert.remaining)
		}).collect::<Vec<String>>().join("\n");
		let message: String = format!("最新价：{:.2}，共命中{}个任务\n{}\n{}", point.price, hits.len(), details, time_text(point.timestamp));
		let result: Result<()> = match self.native.as_ref() {
			Some(native) => native.notify("黄金价位提醒", &message),
			None => Err(anyhow::anyhow!("通知窗口尚未创建")),
		};
		if let Err(error) = result {
			log_error(&format!("Windows 通知提交失败：{error:#}"));
			if let Err(error) = self.store.save_alerts(&self.alerts) { log_error(&format!("恢复提醒次数失败：{error:#}")); }
			return;
		}
		self.alerts = next;
		self.alert_dirty = false;
		self.update_alert_list();
	}

	pub fn open_alert_settings(&mut self) {
		let latest: Option<Point> = self.quote.as_ref().map(|quote: &Quote| quote.point);
		if let Some(native) = self.native.as_mut() {
			if let Err(error) = native.open_alert_settings(&self.alerts, latest) {
				log_error(&format!("打开提醒设置失败：{error:#}"));
				crate::platform::show_error(&error.to_string());
			}
		}
	}

	pub fn configure_alert(&mut self, mut alert: PriceAlert) {
		if alert.id != 0 && !self.alerts.iter().any(|task: &PriceAlert| task.id == alert.id) {
			crate::platform::show_error("该提醒任务已删除，请新建任务");
			return;
		}
		self.seed_alert(&mut alert);
		match self.store.upsert_alert(&alert) {
			Ok(id) => {
				alert.id = id;
				match self.alerts.iter_mut().find(|task: &&mut PriceAlert| task.id == id) {
					Some(task) => *task = alert.clone(),
					None => self.alerts.push(alert.clone()),
				}
				self.update_alert_list();
				if let Some(native) = self.native.as_ref() { native.select_alert(&alert); }
			}
			Err(error) => {
				log_error(&format!("保存提醒设置失败：{error:#}"));
				crate::platform::show_error(&format!("提醒设置未保存：{error:#}"));
			}
		}
	}

	fn seed_alert(&self, alert: &mut PriceAlert) {
		let latest: Option<Point> = self.quote.as_ref().map(|quote: &Quote| quote.point);
		alert.last_timestamp = latest.map(|point: Point| point.timestamp);
		alert.last_price_cents = latest.map(|point: Point| (point.price * 100.0).round() as i64);
	}

	fn update_alert_list(&self) {
		if let Some(native) = self.native.as_ref() { native.update_alert_status(&self.alerts); }
	}

	pub fn select_alert(&self, id: i64) {
		if let Some(alert) = self.alerts.iter().find(|alert: &&PriceAlert| alert.id == id) {
			if let Some(native) = self.native.as_ref() { native.select_alert(alert); }
		}
	}

	pub fn delete_alert(&mut self, id: i64) {
		match self.store.delete_alert(id) {
			Ok(()) => {
				self.alerts.retain(|alert: &PriceAlert| alert.id != id);
				self.update_alert_list();
				if let Some(native) = self.native.as_ref() { native.clear_alert_editor(); }
			}
			Err(error) => crate::platform::show_error(&format!("删除提醒失败：{error:#}")),
		}
	}

	pub fn toggle_alert(&mut self, id: i64) {
		let mut alert: PriceAlert = match self.alerts.iter().find(|alert: &&PriceAlert| alert.id == id) {
			Some(alert) => alert.clone(), None => return,
		};
		alert.enabled = !alert.enabled;
		self.seed_alert(&mut alert);
		match self.store.upsert_alert(&alert) {
			Ok(_) => {
				if let Some(task) = self.alerts.iter_mut().find(|task: &&mut PriceAlert| task.id == id) { *task = alert.clone(); }
				self.update_alert_list();
				if let Some(native) = self.native.as_ref() { native.select_alert(&alert); }
			}
			Err(error) => crate::platform::show_error(&format!("修改提醒状态失败：{error:#}")),
		}
	}

	fn update_hover(&mut self) {
		let mut hover: Option<usize> = None;
		if let Some(native) = self.native.as_ref().filter(|native: &&Native| native.visible) {
			let (x, y): (f32, f32) = native.cursor_position();
			if (LEFT..=LEFT + CHART_WIDTH).contains(&x) && (TOP..=TOP + CHART_HEIGHT).contains(&y) && !self.points.is_empty() {
				let timestamp: f64 = self.session.start as f64 + ((x - LEFT) / CHART_WIDTH) as f64 * ((self.session.end - self.session.start) as f64 - 0.001);
				if timestamp >= self.points[0].timestamp && timestamp <= self.points[self.points.len() - 1].timestamp {
					let index: usize = self.points.partition_point(|point: &Point| point.timestamp < timestamp);
					hover = Some(if index == 0 { 0 } else if index == self.points.len() || timestamp - self.points[index - 1].timestamp <= self.points[index].timestamp - timestamp { index - 1 } else { index });
				}
			}
		}
		if self.hover != hover { self.hover = hover; self.redraw = true; }
	}

	pub fn draw(&mut self) -> Result<()> {
		let native: &Native = self.native.as_ref().expect("窗口已创建");
		let mut status: String = match &self.quote {
			Some(quote) => format!("报价时间 {}", time_text(quote.point.timestamp)),
			None => "正在获取报价…".to_string(),
		};
		if self.quote_error.is_some() {
			status = "报价获取失败，稍后自动重试".to_string();
		} else if self.history_error.is_some() {
			status.push_str(" · 历史失败，重试中");
		} else if !self.history_loaded {
			status.push_str(" · 补充历史中");
		}
		let scene: Scene<'_> = Scene { session: self.session, points: &self.points, quote: self.quote.as_ref(), status: &status, hover: self.hover };
		let chart: Pixmap = self.renderer.chart(scene, native.scale)?;
		let handle: Pixmap = self.renderer.handle(native.scale)?;
		native.present(&chart, &handle)?;
		self.redraw = false;
		Ok(())
	}

	pub fn save(&mut self) {
		match self.store.save(self.session, &self.points) {
			Ok(()) => self.dirty = false,
			Err(error) => log_error(&format!("SQLite 保存失败：{error:#}")),
		}
		match self.store.save_alerts(&self.alerts) {
			Ok(()) => self.alert_dirty = false,
			Err(error) => log_error(&format!("保存提醒状态失败：{error:#}")),
		}
		self.last_save = Instant::now();
	}

	pub fn minimize(&mut self) {
		self.save();
		if let Some(native) = self.native.as_mut() { native.hide(); }
	}

	pub fn restore(&mut self) {
		if let Some(native) = self.native.as_mut() { native.show(); }
		self.redraw = true;
	}

	pub fn exit(&mut self) {
		self.closing = true;
		self.save();
		if let Some(native) = self.native.as_mut() { native.exit(); }
	}
}
