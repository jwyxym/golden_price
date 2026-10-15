use std::{cell::RefCell, collections::VecDeque, ffi::c_void, mem::{size_of, zeroed}, ptr::{null, null_mut}, slice, sync::atomic::{AtomicU32, Ordering}};

use anyhow::{Result, bail};
use tiny_skia::Pixmap;
use windows_sys::Win32::{
	Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
	Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
		CreateBitmap, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject},
	System::LibraryLoader::GetModuleHandleW,
	UI::{
		HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext},
		Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
		Shell::{NIIF_INFO, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIN_BALLOONUSERCLICK, NOTIFYICONDATAW, Shell_NotifyIconW},
		WindowsAndMessaging::*,
	},
};

use crate::{alert::PriceAlert, app::App, drawing::{HEIGHT, WIDTH}, log_error, model::Point, settings};

const TRAY_MESSAGE: u32 = WM_APP + 1;
const SHOW: u32 = 1001;
const HIDE: u32 = 1002;
const EXIT: u32 = 1003;
const ALERT: u32 = 1004;

static TASKBAR_MESSAGE: AtomicU32 = AtomicU32::new(0);

enum NativeEvent {
	Tick,
	DragBegin(POINT),
	DragMove(POINT),
	DragEnd,
	Menu,
	Restore,
	Exit,
	Dpi(f32, RECT),
	RestoreTray,
	ConfigureAlert(PriceAlert),
	SelectAlert(i64),
	DeleteAlert(i64),
	ToggleAlert(i64),
	SettingsClosed(HWND),
}

thread_local! {
	static EVENTS: RefCell<VecDeque<NativeEvent>> = const { RefCell::new(VecDeque::new()) };
}

fn enqueue(event: NativeEvent) {
	EVENTS.with(|events: &RefCell<VecDeque<NativeEvent>>| events.borrow_mut().push_back(event));
}

pub fn submit_alert(alert: PriceAlert) {
	enqueue(NativeEvent::ConfigureAlert(alert));
}

pub fn select_alert(id: i64) {
	enqueue(NativeEvent::SelectAlert(id));
}

pub fn delete_alert(id: i64) {
	enqueue(NativeEvent::DeleteAlert(id));
}

pub fn toggle_alert(id: i64) {
	enqueue(NativeEvent::ToggleAlert(id));
}

pub fn settings_closed(window: HWND) {
	enqueue(NativeEvent::SettingsClosed(window));
}

fn wide(text: &str) -> Vec<u16> {
	text.encode_utf16().chain(Some(0)).collect()
}

fn last_error(message: &str) -> anyhow::Error {
	anyhow::anyhow!("{message}：{}", std::io::Error::last_os_error())
}

pub fn show_error(message: &str) {
	unsafe { MessageBoxW(null_mut(), wide(message).as_ptr(), wide("黄金走势").as_ptr(), MB_OK | MB_ICONERROR); }
}

pub fn run() -> Result<()> {
	unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); }
	let mut app: Box<App> = Box::new(App::new()?);
	let native: Native = Native::create()?;
	let window: HWND = native.chart;
	app.native = Some(native);
	app.draw()?;
	if let Some(native) = app.native.as_mut() { native.show(); }
	unsafe {
		if SetTimer(window, 1, 40, None) == 0 { bail!("无法创建刷新计时器"); }
	}
	app.tick();
	let mut message: MSG = unsafe { zeroed() };
	loop {
		let status: i32 = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
		if status == -1 { return Err(last_error("读取窗口消息失败")); }
		if status == 0 { break; }
		let settings_window: HWND = app.native.as_ref().map(|native: &Native| native.settings).unwrap_or(null_mut());
		if !settings::route_message(settings_window, &message) {
			unsafe { TranslateMessage(&message); DispatchMessageW(&message); }
		}
		// 回调只排队事件；在 DispatchMessage 之外更新状态，避免 Win32 消息重入导致别名。
		loop {
			let event: Option<NativeEvent> = EVENTS.with(|events: &RefCell<VecDeque<NativeEvent>>| events.borrow_mut().pop_front());
			match event {
				Some(event) if !app.closing => process_event(&mut app, event),
				Some(_) => {},
				None => break,
			}
		}
	}
	Ok(())
}

pub struct Native {
	pub chart: HWND,
	pub handle: HWND,
	pub scale: f32,
	pub visible: bool,
	tray: NOTIFYICONDATAW,
	icon: HICON,
	settings: HWND,
}

impl Native {
	fn create() -> Result<Self> {
		unsafe {
			let instance: *mut c_void = GetModuleHandleW(null());
			let class_name: Vec<u16> = wide("GoldenPriceOverlayRust");
			let class: WNDCLASSW = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance,
				hCursor: LoadCursorW(null_mut(), IDC_ARROW), lpszClassName: class_name.as_ptr(), ..zeroed() };
			if RegisterClassW(&class) == 0 { return Err(last_error("注册窗口类失败")); }
			let scale: f32 = GetDpiForSystem().max(96) as f32 / 96.0;
			let mut work: RECT = zeroed();
			if SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0) == 0 {
				return Err(last_error("读取桌面工作区失败"));
			}
			let x: i32 = (work.right - (WIDTH as f32 * scale) as i32 - 24).max(work.left);
			let y: i32 = work.top + 40;
			let chart: HWND = CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
				class_name.as_ptr(), wide("黄金走势").as_ptr(), WS_POPUP, x, y,
				(WIDTH as f32 * scale).round() as i32, (HEIGHT as f32 * scale).round() as i32,
				null_mut(), null_mut(), instance, null());
			if chart.is_null() { return Err(last_error("创建透明窗口失败")); }
			let handle: HWND = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
				class_name.as_ptr(), wide("按住拖动，右键菜单，Esc 退出").as_ptr(), WS_POPUP,
				x + (356.0 * scale) as i32, y + (12.0 * scale) as i32,
				(26.0 * scale).round() as i32, (26.0 * scale).round() as i32, chart, null_mut(), instance, null());
			if handle.is_null() { DestroyWindow(chart); return Err(last_error("创建拖拽按钮失败")); }
			let icon: HICON = create_icon()?;
			let mut tray: NOTIFYICONDATAW = zeroed();
			tray.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
			tray.hWnd = chart;
			tray.uID = 1;
			tray.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
			tray.uCallbackMessage = TRAY_MESSAGE;
			tray.hIcon = icon;
			TASKBAR_MESSAGE.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);
			let mut native: Self = Self { chart, handle, scale, visible: true, tray, icon, settings: null_mut() };
			native.set_tooltip("黄金走势 · 点击恢复，右键菜单");
			if Shell_NotifyIconW(NIM_ADD, &native.tray) == 0 {
				return Err(last_error("创建系统托盘图标失败"));
			}
			Ok(native)
		}
	}

	pub fn present(&self, chart: &Pixmap, handle: &Pixmap) -> Result<()> {
		present(self.chart, chart)?;
		present(self.handle, handle)
	}

	pub fn cursor_position(&self) -> (f32, f32) {
		unsafe {
			let mut cursor: POINT = zeroed();
			let mut rect: RECT = zeroed();
			if GetCursorPos(&mut cursor) == 0 || GetWindowRect(self.chart, &mut rect) == 0 { return (-1.0, -1.0); }
			((cursor.x - rect.left) as f32 / self.scale, (cursor.y - rect.top) as f32 / self.scale)
		}
	}

	pub fn set_tooltip(&mut self, text: &str) {
		self.tray.szTip.fill(0);
		text.encode_utf16().take(127).enumerate().for_each(|(index, value): (usize, u16)| { self.tray.szTip[index] = value; });
		unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.tray); }
	}

	pub fn notify(&self, title: &str, message: &str) -> Result<()> {
		let mut notification: NOTIFYICONDATAW = self.tray;
		notification.uFlags = NIF_INFO;
		notification.szInfoTitle.fill(0);
		notification.szInfo.fill(0);
		title.encode_utf16().take(63).enumerate().for_each(|(index, value): (usize, u16)| { notification.szInfoTitle[index] = value; });
		message.encode_utf16().take(255).enumerate().for_each(|(index, value): (usize, u16)| { notification.szInfo[index] = value; });
		notification.dwInfoFlags = NIIF_INFO;
		if unsafe { Shell_NotifyIconW(NIM_MODIFY, &notification) } == 0 { return Err(last_error("提交系统通知失败")); }
		Ok(())
	}

	pub fn open_alert_settings(&mut self, alerts: &[PriceAlert], latest: Option<Point>) -> Result<()> {
		unsafe {
			if !self.settings.is_null() && IsWindow(self.settings) != 0 {
				settings::update_tasks(self.settings, alerts);
				ShowWindow(self.settings, SW_RESTORE);
				SetForegroundWindow(self.settings);
				return Ok(());
			}
		}
		self.settings = settings::open(alerts, latest)?;
		Ok(())
	}

	pub fn update_alert_status(&self, alerts: &[PriceAlert]) {
		settings::update_tasks(self.settings, alerts);
	}

	pub fn select_alert(&self, alert: &PriceAlert) {
		settings::select_task(self.settings, alert);
	}

	pub fn clear_alert_editor(&self) {
		settings::clear_editor(self.settings);
	}

	pub fn show(&mut self) {
		self.visible = true;
		unsafe {
			ShowWindow(self.chart, SW_SHOWNOACTIVATE);
			ShowWindow(self.handle, SW_SHOWNOACTIVATE);
			SetWindowPos(self.chart, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
			self.move_handle();
		}
	}

	pub fn hide(&mut self) {
		self.visible = false;
		unsafe { ShowWindow(self.handle, SW_HIDE); ShowWindow(self.chart, SW_HIDE); }
	}

	pub fn exit(&mut self) {
		self.hide();
		unsafe { KillTimer(self.chart, 1); PostQuitMessage(0); }
	}

	fn move_to(&self, x: i32, y: i32) {
		unsafe {
			SetWindowPos(self.chart, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
			self.move_handle();
		}
	}

	fn move_handle(&self) {
		unsafe {
			let mut rect: RECT = zeroed();
			GetWindowRect(self.chart, &mut rect);
			SetWindowPos(self.handle, HWND_TOPMOST, rect.left + (356.0 * self.scale).round() as i32,
				rect.top + (12.0 * self.scale).round() as i32, (26.0 * self.scale).round() as i32,
				(26.0 * self.scale).round() as i32, SWP_NOACTIVATE);
		}
	}
}

impl Drop for Native {
	fn drop(&mut self) {
		unsafe {
			if !self.settings.is_null() && IsWindow(self.settings) != 0 { DestroyWindow(self.settings); }
			Shell_NotifyIconW(NIM_DELETE, &self.tray);
			DestroyWindow(self.handle);
			DestroyWindow(self.chart);
			DestroyIcon(self.icon);
		}
	}
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
	unsafe {
		if message != 0 && message == TASKBAR_MESSAGE.load(Ordering::Relaxed) {
			enqueue(NativeEvent::RestoreTray);
			return 0;
		}
		match message {
			WM_TIMER => { enqueue(NativeEvent::Tick); 0 }
			WM_ERASEBKGND => 1,
			WM_LBUTTONDOWN => {
				let mut cursor: POINT = zeroed();
				GetCursorPos(&mut cursor);
				enqueue(NativeEvent::DragBegin(cursor));
				SetCapture(window);
				0
			}
			WM_MOUSEMOVE => {
				let mut cursor: POINT = zeroed();
				GetCursorPos(&mut cursor);
				enqueue(NativeEvent::DragMove(cursor));
				0
			}
			WM_LBUTTONUP => { enqueue(NativeEvent::DragEnd); ReleaseCapture(); 0 }
			WM_CAPTURECHANGED => { enqueue(NativeEvent::DragEnd); 0 }
			WM_SETCURSOR => {
				if !GetWindow(window, GW_OWNER).is_null() && (lparam as u32 & 0xffff) == HTCLIENT {
					SetCursor(LoadCursorW(null_mut(), IDC_SIZEALL));
					1
				} else { DefWindowProcW(window, message, wparam, lparam) }
			}
			WM_CONTEXTMENU => { enqueue(NativeEvent::Menu); 0 }
			TRAY_MESSAGE => {
				match lparam as u32 {
					WM_LBUTTONUP | WM_LBUTTONDBLCLK | NIN_BALLOONUSERCLICK => enqueue(NativeEvent::Restore),
					WM_RBUTTONUP | WM_CONTEXTMENU => enqueue(NativeEvent::Menu),
					_ => {}
				}
				0
			}
			WM_KEYDOWN if wparam == 27 => { enqueue(NativeEvent::Exit); 0 }
			WM_CLOSE => { enqueue(NativeEvent::Exit); 0 }
			WM_DPICHANGED => {
				let is_chart: bool = GetWindow(window, GW_OWNER).is_null();
				if is_chart && lparam != 0 {
					let rect: RECT = *(lparam as *const RECT);
					enqueue(NativeEvent::Dpi((wparam as u16).max(96) as f32 / 96.0, rect));
				}
				0
			}
			_ => DefWindowProcW(window, message, wparam, lparam),
		}
	}
}

fn process_event(app: &mut App, event: NativeEvent) {
	match event {
		NativeEvent::Tick => app.tick(),
		NativeEvent::Restore => app.restore(),
		NativeEvent::Exit => app.exit(),
		NativeEvent::DragEnd => app.drag_offset = None,
		NativeEvent::DragBegin(cursor) => {
			let mut rect: RECT = unsafe { zeroed() };
			if let Some(native) = app.native.as_ref() {
				unsafe { GetWindowRect(native.chart, &mut rect); }
				app.drag_offset = Some((cursor.x - rect.left, cursor.y - rect.top));
			}
		}
		NativeEvent::DragMove(cursor) => {
			if let Some((dx, dy)) = app.drag_offset {
				if let Some(native) = app.native.as_ref() { native.move_to(cursor.x - dx, cursor.y - dy); }
			}
		}
		NativeEvent::Dpi(scale, rect) => {
			if let Some(native) = app.native.as_mut() { native.scale = scale; native.move_to(rect.left, rect.top); }
			if let Err(error) = app.draw() { log_error(&format!("DPI 更新失败：{error:#}")); }
		}
		NativeEvent::RestoreTray => {
			if let Some(native) = app.native.as_ref() { unsafe { Shell_NotifyIconW(NIM_ADD, &native.tray); } }
		}
		NativeEvent::ConfigureAlert(alert) => app.configure_alert(alert),
		NativeEvent::SelectAlert(id) => app.select_alert(id),
		NativeEvent::DeleteAlert(id) => app.delete_alert(id),
		NativeEvent::ToggleAlert(id) => app.toggle_alert(id),
		NativeEvent::SettingsClosed(window) => {
			if let Some(native) = app.native.as_mut() {
				if native.settings == window { native.settings = null_mut(); }
			}
		}
		NativeEvent::Menu => {
			let chart: HWND = match app.native.as_ref() { Some(native) => native.chart, None => return };
			unsafe { KillTimer(chart, 1); }
			let command: u32 = popup_menu(chart);
			match command { SHOW => app.restore(), HIDE => app.minimize(), EXIT => app.exit(), ALERT => app.open_alert_settings(), _ => {} }
			if !app.closing { unsafe { SetTimer(chart, 1, 40, None); } }
		}
	}
}

fn popup_menu(window: HWND) -> u32 {
	unsafe {
		let menu: HMENU = CreatePopupMenu();
		if menu.is_null() { return 0; }
		AppendMenuW(menu, MF_STRING, SHOW as usize, wide("显示悬浮窗").as_ptr());
		AppendMenuW(menu, MF_STRING, HIDE as usize, wide("最小化到托盘").as_ptr());
		AppendMenuW(menu, MF_STRING, ALERT as usize, wide("提醒任务列表…").as_ptr());
		AppendMenuW(menu, MF_SEPARATOR, 0, null());
		AppendMenuW(menu, MF_STRING, EXIT as usize, wide("退出程序").as_ptr());
		let mut cursor: POINT = zeroed();
		GetCursorPos(&mut cursor);
		SetForegroundWindow(window);
		SetCursor(LoadCursorW(null_mut(), IDC_ARROW));
		let result: i32 = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, cursor.x, cursor.y, 0, window, null());
		PostMessageW(window, WM_NULL, 0, 0);
		DestroyMenu(menu);
		result as u32
	}
}

struct Surface {
	dc: HDC,
	bitmap: HBITMAP,
	previous: HGDIOBJ,
}

impl Surface {
	fn new(width: u32, height: u32, pixels: &[u8]) -> Result<Self> {
		unsafe {
			let dc: HDC = CreateCompatibleDC(null_mut());
			if dc.is_null() { return Err(last_error("创建绘图上下文失败")); }
			let info: BITMAPINFO = BITMAPINFO { bmiHeader: BITMAPINFOHEADER {
				biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: width as i32, biHeight: -(height as i32),
				biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..zeroed()
			}, ..zeroed() };
			let mut bits: *mut c_void = null_mut();
			let bitmap: HBITMAP = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
			if bitmap.is_null() || bits.is_null() { DeleteDC(dc); return Err(last_error("创建透明位图失败")); }
			let previous: HGDIOBJ = SelectObject(dc, bitmap);
			let output: &mut [u8] = slice::from_raw_parts_mut(bits.cast(), pixels.len());
			pixels.chunks_exact(4).zip(output.chunks_exact_mut(4)).for_each(|(source, target): (&[u8], &mut [u8])| {
				target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
			});
			Ok(Self { dc, bitmap, previous })
		}
	}
}

impl Drop for Surface {
	fn drop(&mut self) {
		unsafe { SelectObject(self.dc, self.previous); DeleteObject(self.bitmap); DeleteDC(self.dc); }
	}
}

fn present(window: HWND, pixmap: &Pixmap) -> Result<()> {
	let surface: Surface = Surface::new(pixmap.width(), pixmap.height(), pixmap.data())?;
	let size: SIZE = SIZE { cx: pixmap.width() as i32, cy: pixmap.height() as i32 };
	let source: POINT = POINT { x: 0, y: 0 };
	let blend: BLENDFUNCTION = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
	if unsafe { UpdateLayeredWindow(window, null_mut(), null(), &size, surface.dc, &source, 0, &blend, ULW_ALPHA) } == 0 {
		return Err(last_error("提交透明窗口像素失败"));
	}
	Ok(())
}

fn create_icon() -> Result<HICON> {
	let mut pixels: Vec<u8> = vec![0; 32 * 32 * 4];
	let mut y: usize = 0;
	while y < 32 {
		let mut x: usize = 0;
		while x < 32 {
			let offset: usize = (y * 32 + x) * 4;
			let on_line: bool = x > 4 && x < 28 && ((29 - x) as i32 - y as i32).abs() < 2;
			pixels[offset..offset + 4].copy_from_slice(if on_line { &[241, 192, 96, 255] } else { &[20, 26, 38, 255] });
			x += 1;
		}
		y += 1;
	}
	let surface: Surface = Surface::new(32, 32, &pixels)?;
	unsafe {
		let mask: HBITMAP = CreateBitmap(32, 32, 1, 1, null());
		if mask.is_null() { return Err(last_error("创建图标蒙版失败")); }
		let info: ICONINFO = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: surface.bitmap };
		let icon: HICON = CreateIconIndirect(&info);
		DeleteObject(mask);
		if icon.is_null() { return Err(last_error("创建托盘图标失败")); }
		Ok(icon)
	}
}
