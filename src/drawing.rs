use std::{collections::HashMap, fs, path::PathBuf};

use anyhow::{Context, Result};
use fontdue::{Font, FontSettings, Metrics};
use tiny_skia::{Color, FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point as SkPoint, Rect, SpreadMode, Stroke, Transform};

use crate::model::{Point, Quote, Session, time_text};

pub const WIDTH: i32 = 400;
pub const HEIGHT: i32 = 285;
pub const LEFT: f32 = 26.0;
pub const TOP: f32 = 112.0;
pub const CHART_WIDTH: f32 = 292.0;
pub const CHART_HEIGHT: f32 = 130.0;
const GOLD: [u8; 4] = [241, 192, 96, 255];
const MUTED: [u8; 4] = [141, 152, 172, 255];

pub struct Scene<'a> {
	pub session: Session,
	pub points: &'a [Point],
	pub quote: Option<&'a Quote>,
	pub status: &'a str,
	pub hover: Option<usize>,
}

struct Glyph {
	metrics: Metrics,
	bitmap: Vec<u8>,
}

pub struct Renderer {
	font: Font,
	glyphs: HashMap<(char, u32), Glyph>,
}

impl Renderer {
	pub fn new() -> Result<Self> {
		let root: PathBuf = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()));
		let file: PathBuf = ["msyh.ttc", "simhei.ttf", "segoeui.ttf"].iter()
			.map(|name: &&str| root.join("Fonts").join(name)).find(|path: &PathBuf| path.is_file())
			.context("未找到 Windows 字体")?;
		let font: Font = Font::from_bytes(fs::read(file)?, FontSettings::default()).map_err(anyhow::Error::msg)?;
		Ok(Self { font, glyphs: HashMap::new() })
	}

	pub fn chart(&mut self, scene: Scene<'_>, scale: f32) -> Result<Pixmap> {
		let mut pixmap: Pixmap = Pixmap::new((WIDTH as f32 * scale).round() as u32, (HEIGHT as f32 * scale).round() as u32)
			.context("图表像素分配失败")?;
		pixmap.fill(Color::from_rgba(0.0, 0.0, 0.0, 0.5).expect("有效背景颜色"));
		let mut canvas: Canvas<'_> = Canvas { pixmap, renderer: self, scale };
		match scene.quote {
			Some(quote) => {
				canvas.text(18.0, 42.0, &format!("{:.2}", quote.point.price), 28.0, GOLD);
				canvas.text(18.0, 80.0, &format!("较昨收 {} ({})", quote.change, quote.rate), 11.0, MUTED);
			}
			None => canvas.text(18.0, 42.0, "--", 28.0, GOLD),
		}
		canvas.text(LEFT, 252.0, "09:10", 10.0, MUTED);
		canvas.text(250.0, 252.0, "次日 01:59", 10.0, MUTED);
		let status: String = scene.status.chars().take(38).collect();
		canvas.text(18.0, 271.0, &status, 9.0, MUTED);
		if scene.points.is_empty() {
			canvas.text(108.0, 170.0, "等待交易时段的报价…", 12.0, MUTED);
			return Ok(canvas.pixmap);
		}
		let maximum: &Point = scene.points.iter().max_by(|a: &&Point, b: &&Point| a.price.total_cmp(&b.price)).context("无最高点")?;
		let minimum: &Point = scene.points.iter().min_by(|a: &&Point, b: &&Point| a.price.total_cmp(&b.price)).context("无最低点")?;
		let padding: f64 = ((maximum.price - minimum.price) * 0.2).max(0.05);
		let low: f64 = minimum.price - padding;
		let high: f64 = maximum.price + padding;
		let position: &dyn Fn(&Point) -> (f32, f32) = &|point: &Point| -> (f32, f32) {
			(LEFT + scene.session.fraction(point.timestamp) * CHART_WIDTH,
			 TOP + CHART_HEIGHT - ((point.price - low) / (high - low)) as f32 * CHART_HEIGHT)
		};
		let mut index: usize = 0;
		while index < 3 {
			let y: f32 = TOP + CHART_HEIGHT * index as f32 / 2.0;
			let mut dash: f32 = LEFT;
			while dash < LEFT + CHART_WIDTH {
				canvas.line(dash, y, (dash + 4.0).min(LEFT + CHART_WIDTH), y, 1.0, [70, 80, 100, 130]);
				dash += 8.0;
			}
			canvas.text(LEFT + CHART_WIDTH + 6.0, y - 6.0, &format!("{:.2}", high - (high - low) * index as f64 / 2.0), 10.0, MUTED);
			index += 1;
		}
		let mut builder: PathBuilder = PathBuilder::new();
		let (first_x, first_y): (f32, f32) = position(&scene.points[0]);
		builder.move_to(first_x, first_y);
		scene.points.iter().skip(1).for_each(|point: &Point| {
			let (x, y): (f32, f32) = position(point);
			builder.line_to(x, y);
		});
		let last: &Point = scene.points.last().context("无末尾点")?;
		let (last_x, last_y): (f32, f32) = position(last);
		if scene.points.len() > 1 {
			let path: Path = builder.clone().finish().context("曲线路径无效")?;
			builder.line_to(last_x, TOP + CHART_HEIGHT);
			builder.line_to(first_x, TOP + CHART_HEIGHT);
			builder.close();
			let fill: Path = builder.finish().context("填充路径无效")?;
			let mut paint: Paint<'_> = Paint::default();
			paint.shader = LinearGradient::new(SkPoint::from_xy(0.0, TOP), SkPoint::from_xy(0.0, TOP + CHART_HEIGHT),
				vec![GradientStop::new(0.0, Color::from_rgba8(241, 192, 96, 65)), GradientStop::new(1.0, Color::TRANSPARENT)],
				SpreadMode::Pad, Transform::identity()).context("渐变无效")?;
			canvas.pixmap.fill_path(&fill, &paint, FillRule::Winding, Transform::from_scale(scale, scale), None);
			canvas.stroke(&path, 2.0, GOLD);
		}
		canvas.dot(last_x, last_y, 3.0, GOLD);
		if maximum.price == minimum.price {
			canvas.marker(position(maximum), &format!("最高/最低 {:.2}", maximum.price), GOLD, -26.0);
		} else {
			canvas.marker(position(maximum), &format!("最高 {:.2}", maximum.price), [240, 113, 120, 255], -26.0);
			canvas.marker(position(minimum), &format!("最低 {:.2}", minimum.price), [86, 201, 154, 255], 8.0);
		}
		if let Some(point) = scene.hover.and_then(|index: usize| scene.points.get(index)) {
			let (x, y): (f32, f32) = position(point);
			canvas.line(x, TOP, x, TOP + CHART_HEIGHT, 1.0, [181, 191, 206, 180]);
			canvas.line(LEFT, y, LEFT + CHART_WIDTH, y, 1.0, [181, 191, 206, 180]);
			canvas.dot(x, y, 4.0, [255, 255, 255, 255]);
			let bx: f32 = if x + 150.0 <= LEFT + CHART_WIDTH { x + 12.0 } else { (x - 150.0).max(LEFT) };
			let by: f32 = (y - 56.0).clamp(TOP, TOP + CHART_HEIGHT - 46.0);
			canvas.rect(bx, by, 138.0, 46.0, [20, 26, 38, 245]);
			canvas.text(bx + 8.0, by + 6.0, &format!("价格：{:.2}", point.price), 11.0, GOLD);
			canvas.text(bx + 8.0, by + 25.0, &time_text(point.timestamp), 10.0, [233, 237, 245, 255]);
		}
		Ok(canvas.pixmap)
	}

	pub fn handle(&mut self, scale: f32) -> Result<Pixmap> {
		let pixmap: Pixmap = Pixmap::new((26.0 * scale).round() as u32, (26.0 * scale).round() as u32).context("按钮像素分配失败")?;
		let mut canvas: Canvas<'_> = Canvas { pixmap, renderer: self, scale };
		canvas.rect(0.0, 0.0, 26.0, 26.0, [53, 64, 87, 240]);
		canvas.line(6.0, 13.0, 20.0, 13.0, 1.5, MUTED);
		canvas.line(13.0, 6.0, 13.0, 20.0, 1.5, MUTED);
		canvas.line(6.0, 13.0, 9.0, 10.0, 1.5, MUTED);
		canvas.line(6.0, 13.0, 9.0, 16.0, 1.5, MUTED);
		canvas.line(20.0, 13.0, 17.0, 10.0, 1.5, MUTED);
		canvas.line(20.0, 13.0, 17.0, 16.0, 1.5, MUTED);
		Ok(canvas.pixmap)
	}
}

struct Canvas<'a> {
	pixmap: Pixmap,
	renderer: &'a mut Renderer,
	scale: f32,
}

impl Canvas<'_> {
	fn paint(color: [u8; 4]) -> Paint<'static> {
		let mut paint: Paint<'static> = Paint::default();
		paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
		paint
	}

	fn stroke(&mut self, path: &Path, width: f32, color: [u8; 4]) {
		let stroke: Stroke = Stroke { width, ..Stroke::default() };
		self.pixmap.stroke_path(path, &Self::paint(color), &stroke, Transform::from_scale(self.scale, self.scale), None);
	}

	fn line(&mut self, x: f32, y: f32, end_x: f32, end_y: f32, width: f32, color: [u8; 4]) {
		let mut builder: PathBuilder = PathBuilder::new();
		builder.move_to(x, y);
		builder.line_to(end_x, end_y);
		if let Some(path) = builder.finish() {
			self.stroke(&path, width, color);
		}
	}

	fn rect(&mut self, x: f32, y: f32, width: f32, height: f32, color: [u8; 4]) {
		if let Some(rect) = Rect::from_xywh(x, y, width, height) {
			self.pixmap.fill_rect(rect, &Self::paint(color), Transform::from_scale(self.scale, self.scale), None);
		}
	}

	fn dot(&mut self, x: f32, y: f32, radius: f32, color: [u8; 4]) {
		if let Some(path) = PathBuilder::from_circle(x, y, radius) {
			self.pixmap.fill_path(&path, &Self::paint(color), FillRule::Winding, Transform::from_scale(self.scale, self.scale), None);
		}
	}

	fn marker(&mut self, position: (f32, f32), text: &str, color: [u8; 4], offset: f32) {
		let (x, y): (f32, f32) = position;
		let width: f32 = text.chars().map(|character: char| self.renderer.font.metrics(character, 10.0).advance_width).sum::<f32>() + 12.0;
		let bx: f32 = (x - width / 2.0).clamp(LEFT, LEFT + CHART_WIDTH - width);
		let by: f32 = (y + offset).clamp(TOP - 12.0, TOP + CHART_HEIGHT - 22.0);
		self.dot(x, y, 4.0, color);
		self.rect(bx, by, width, 22.0, [20, 26, 38, 220]);
		self.text(bx + 6.0, by + 5.0, text, 10.0, color);
	}

	fn text(&mut self, x: f32, y: f32, text: &str, size: f32, color: [u8; 4]) {
		let pixels: f32 = size * self.scale;
		let mut cursor: f32 = x * self.scale;
		let baseline: f32 = (y + size) * self.scale;
		text.chars().for_each(|character: char| {
			let glyph: &Glyph = self.renderer.glyphs.entry((character, pixels.to_bits())).or_insert_with(|| {
				let (metrics, bitmap): (Metrics, Vec<u8>) = self.renderer.font.rasterize(character, pixels);
				Glyph { metrics, bitmap }
			});
			let left: i32 = cursor.round() as i32 + glyph.metrics.xmin;
			let top: i32 = baseline.round() as i32 - glyph.metrics.height as i32 - glyph.metrics.ymin;
			let width: usize = self.pixmap.width() as usize;
			let height: i32 = self.pixmap.height() as i32;
			let buffer: &mut [u8] = self.pixmap.data_mut();
			glyph.bitmap.iter().enumerate().for_each(|(index, coverage): (usize, &u8)| {
				let px: i32 = left + (index % glyph.metrics.width) as i32;
				let py: i32 = top + (index / glyph.metrics.width) as i32;
				if px < 0 || py < 0 || px >= width as i32 || py >= height {
					return;
				}
				let alpha: u32 = *coverage as u32 * color[3] as u32 / 255;
				let offset: usize = (py as usize * width + px as usize) * 4;
				let mut channel: usize = 0;
				while channel < 3 {
					buffer[offset + channel] = ((color[channel] as u32 * alpha + buffer[offset + channel] as u32 * (255 - alpha) + 127) / 255) as u8;
					channel += 1;
				}
				buffer[offset + 3] = (alpha + buffer[offset + 3] as u32 * (255 - alpha) / 255).min(255) as u8;
			});
			cursor += glyph.metrics.advance_width;
		});
	}
}
