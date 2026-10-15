use std::{mem::zeroed, ptr::{null, null_mut}};

use anyhow::{Result, bail};
use windows_sys::Win32::{
	Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
	Graphics::Gdi::{COLOR_WINDOW, DEFAULT_GUI_FONT, GetStockObject, HBRUSH, HGDIOBJ, InvalidateRect},
	System::LibraryLoader::GetModuleHandleW,
	UI::{Controls::{BST_CHECKED, BST_UNCHECKED, EM_SETLIMITTEXT},
		HiDpi::GetDpiForSystem, Input::KeyboardAndMouse::SetFocus, WindowsAndMessaging::*},
};

use crate::{alert::{Direction, PriceAlert}, model::Point, platform};

const PRICE: i32 = 201;
const COUNT: i32 = 202;
const ENABLED: i32 = 203;
const STATUS: i32 = 204;
const TASKS: i32 = 205;
const DIRECTION: i32 = 206;
const NEW: i32 = 207;
const DELETE: i32 = 208;
const TOGGLE: i32 = 209;
const SAVE: i32 = 1;
const CANCEL: i32 = 2;

fn wide(text: &str) -> Vec<u16> {
	text.encode_utf16().chain(Some(0)).collect()
}

fn control(parent: HWND, class: &str, text: &str, id: i32, rect: [i32; 4], style: u32, scale: f32) -> Result<HWND> {
	unsafe {
		let window: HWND = CreateWindowExW(if class == "EDIT" || class == "LISTBOX" { WS_EX_CLIENTEDGE } else { 0 },
			wide(class).as_ptr(), wide(text).as_ptr(), WS_CHILD | WS_VISIBLE | style,
			(rect[0] as f32 * scale) as i32, (rect[1] as f32 * scale) as i32,
			(rect[2] as f32 * scale) as i32, (rect[3] as f32 * scale) as i32,
			parent, id as usize as HMENU, GetModuleHandleW(null()), null());
		if window.is_null() { bail!("创建提醒任务控件失败：{}", std::io::Error::last_os_error()); }
		let font: HGDIOBJ = GetStockObject(DEFAULT_GUI_FONT);
		SendMessageW(window, WM_SETFONT, font as WPARAM, 1);
		Ok(window)
	}
}

pub fn open(alerts: &[PriceAlert], latest: Option<Point>) -> Result<HWND> {
	unsafe {
		let class_name: Vec<u16> = wide("GoldenPriceAlertSettings");
		let class: WNDCLASSW = WNDCLASSW {
			lpfnWndProc: Some(window_proc), hInstance: GetModuleHandleW(null()),
			hCursor: LoadCursorW(null_mut(), IDC_ARROW), hbrBackground: (COLOR_WINDOW + 1) as HBRUSH,
			lpszClassName: class_name.as_ptr(), ..zeroed()
		};
		RegisterClassW(&class);
		let scale: f32 = GetDpiForSystem().max(96) as f32 / 96.0;
		let style: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
		let mut rect: RECT = RECT { left: 0, top: 0, right: (680.0 * scale) as i32, bottom: (450.0 * scale) as i32 };
		AdjustWindowRectEx(&mut rect, style, 0, WS_EX_TOPMOST | WS_EX_CONTROLPARENT);
		let mut work: RECT = zeroed();
		SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);
		let window: HWND = CreateWindowExW(WS_EX_TOPMOST | WS_EX_CONTROLPARENT, class_name.as_ptr(), wide("价位提醒任务列表").as_ptr(), style,
			work.left + (work.right - work.left - rect.right + rect.left) / 2,
			work.top + (work.bottom - work.top - rect.bottom + rect.top) / 2,
			rect.right - rect.left, rect.bottom - rect.top, null_mut(), null_mut(), GetModuleHandleW(null()), null());
		if window.is_null() { bail!("创建提醒任务窗口失败：{}", std::io::Error::last_os_error()); }
		let result: Result<()> = (|| -> Result<()> {
			control(window, "STATIC", "提醒任务（编号 / 目标价 / 方向 / 状态 / 剩余次数）", 0, [20, 16, 640, 22], 0, scale)?;
			control(window, "LISTBOX", "", TASKS, [20, 44, 640, 160], WS_TABSTOP | WS_VSCROLL | LBS_NOTIFY as u32 | LBS_NOINTEGRALHEIGHT as u32, scale)?;
			control(window, "BUTTON", "新建任务", NEW, [20, 214, 100, 28], WS_TABSTOP, scale)?;
			control(window, "BUTTON", "暂停 / 继续", TOGGLE, [132, 214, 110, 28], WS_TABSTOP, scale)?;
			control(window, "BUTTON", "删除任务", DELETE, [254, 214, 100, 28], WS_TABSTOP, scale)?;
			control(window, "STATIC", "提醒价格", 0, [20, 263, 80, 24], 0, scale)?;
			let price: HWND = control(window, "EDIT", "", PRICE, [100, 258, 135, 28], WS_TABSTOP | ES_AUTOHSCROLL as u32, scale)?;
			SendMessageW(price, EM_SETLIMITTEXT, 20, 0);
			control(window, "STATIC", "通知次数", 0, [260, 263, 80, 24], 0, scale)?;
			let count: HWND = control(window, "EDIT", "1", COUNT, [340, 258, 135, 28], WS_TABSTOP | ES_NUMBER as u32, scale)?;
			SendMessageW(count, EM_SETLIMITTEXT, 10, 0);
			control(window, "BUTTON", "启用任务", ENABLED, [510, 258, 140, 28], WS_TABSTOP | BS_AUTOCHECKBOX as u32, scale)?;
			control(window, "STATIC", "触发方向", 0, [20, 307, 80, 24], 0, scale)?;
			let direction: HWND = control(window, "COMBOBOX", "", DIRECTION, [100, 302, 180, 140], WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32, scale)?;
			["双向提醒", "涨到提醒", "跌到提醒"].iter().for_each(|label: &&str| {
				SendMessageW(direction, CB_ADDSTRING, 0, wide(label).as_ptr() as LPARAM);
			});
			control(window, "STATIC", "", STATUS, [300, 307, 360, 24], 0, scale)?;
			control(window, "STATIC", "选中任务可编辑；保存修改会重新计数，暂停 / 继续保留剩余次数。\n每次触及或跨过目标价提醒一次，持续停留不重复；次数用完后停止。", 0, [20, 352, 640, 44], 0, scale)?;
			control(window, "BUTTON", "添加任务", SAVE, [450, 408, 100, 28], WS_TABSTOP | BS_DEFPUSHBUTTON as u32, scale)?;
			control(window, "BUTTON", "关闭", CANCEL, [560, 408, 100, 28], WS_TABSTOP, scale)?;
			Ok(())
		})();
		if let Err(error) = result { DestroyWindow(window); return Err(error); }
		clear_editor(window);
		if let Some(point) = latest { SetDlgItemTextW(window, PRICE, wide(&format!("{:.2}", point.price)).as_ptr()); }
		update_tasks(window, alerts);
		if let Some(alert) = alerts.first() { select_task(window, alert); }
		ShowWindow(window, SW_SHOW);
		SetForegroundWindow(window);
		SetFocus(GetDlgItem(window, if alerts.is_empty() { PRICE } else { TASKS }));
		Ok(window)
	}
}

fn text(window: HWND, id: i32) -> String {
	let mut buffer: [u16; 128] = [0; 128];
	let length: u32 = unsafe { GetDlgItemTextW(window, id, buffer.as_mut_ptr(), buffer.len() as i32) };
	String::from_utf16_lossy(&buffer[..length as usize])
}

fn selected_id(window: HWND) -> Option<i64> {
	unsafe {
		let list: HWND = GetDlgItem(window, TASKS);
		let index: LRESULT = SendMessageW(list, LB_GETCURSEL, 0, 0);
		if index < 0 { None } else { Some(SendMessageW(list, LB_GETITEMDATA, index as WPARAM, 0) as i64) }
	}
}

pub fn clear_editor(window: HWND) {
	unsafe {
		if window.is_null() || IsWindow(window) == 0 { return; }
		SetWindowLongPtrW(window, GWLP_USERDATA, 0);
		SendMessageW(GetDlgItem(window, TASKS), LB_SETCURSEL, usize::MAX, 0);
		SetDlgItemTextW(window, PRICE, wide("").as_ptr());
		SetDlgItemTextW(window, COUNT, wide("1").as_ptr());
		SendMessageW(GetDlgItem(window, DIRECTION), CB_SETCURSEL, 0, 0);
		SendMessageW(GetDlgItem(window, ENABLED), BM_SETCHECK, BST_CHECKED as WPARAM, 0);
		SetDlgItemTextW(window, STATUS, wide("正在新建任务").as_ptr());
		SetDlgItemTextW(window, SAVE, wide("添加任务").as_ptr());
	}
}

fn state_text(alert: &PriceAlert) -> &'static str {
	if !alert.enabled { "已暂停" } else if alert.remaining == 0 { "已完成" } else { "监测中" }
}

pub fn update_tasks(window: HWND, alerts: &[PriceAlert]) {
	unsafe {
		if window.is_null() || IsWindow(window) == 0 { return; }
		let selected: Option<i64> = selected_id(window);
		let list: HWND = GetDlgItem(window, TASKS);
		let top: LRESULT = SendMessageW(list, LB_GETTOPINDEX, 0, 0);
		SendMessageW(list, WM_SETREDRAW, 0, 0);
		SendMessageW(list, LB_RESETCONTENT, 0, 0);
		alerts.iter().for_each(|alert: &PriceAlert| {
			let label: String = format!("#{}  |  {}  |  {}  |  {}  |  剩余 {} / {} 次", alert.id, alert.target_text(), alert.direction.label(), state_text(alert), alert.remaining, alert.limit);
			let index: LRESULT = SendMessageW(list, LB_ADDSTRING, 0, wide(&label).as_ptr() as LPARAM);
			if index >= 0 {
				SendMessageW(list, LB_SETITEMDATA, index as WPARAM, alert.id as LPARAM);
				if selected == Some(alert.id) { SendMessageW(list, LB_SETCURSEL, index as WPARAM, 0); }
			}
		});
		SendMessageW(list, LB_SETTOPINDEX, top.max(0) as WPARAM, 0);
		SendMessageW(list, WM_SETREDRAW, 1, 0);
		InvalidateRect(list, null(), 1);
		let editing: i64 = GetWindowLongPtrW(window, GWLP_USERDATA) as i64;
		if let Some(alert) = alerts.iter().find(|alert: &&PriceAlert| alert.id == editing) {
			SetDlgItemTextW(window, STATUS, wide(&format!("{}，剩余 {} / {} 次", state_text(alert), alert.remaining, alert.limit)).as_ptr());
		}
	}
}

pub fn select_task(window: HWND, alert: &PriceAlert) {
	unsafe {
		if window.is_null() || IsWindow(window) == 0 { return; }
		SetWindowLongPtrW(window, GWLP_USERDATA, alert.id as isize);
		SetDlgItemTextW(window, PRICE, wide(&alert.target_text()).as_ptr());
		SetDlgItemTextW(window, COUNT, wide(&alert.limit.to_string()).as_ptr());
		SendMessageW(GetDlgItem(window, ENABLED), BM_SETCHECK, if alert.enabled { BST_CHECKED } else { BST_UNCHECKED } as WPARAM, 0);
		SendMessageW(GetDlgItem(window, DIRECTION), CB_SETCURSEL, match alert.direction { Direction::Both => 0, Direction::Up => 1, Direction::Down => 2 }, 0);
		SetDlgItemTextW(window, STATUS, wide(&format!("{}，剩余 {} / {} 次", state_text(alert), alert.remaining, alert.limit)).as_ptr());
		SetDlgItemTextW(window, SAVE, wide("保存修改").as_ptr());
		let list: HWND = GetDlgItem(window, TASKS);
		let count: LRESULT = SendMessageW(list, LB_GETCOUNT, 0, 0);
		let mut index: usize = 0;
		while index < count.max(0) as usize {
			if SendMessageW(list, LB_GETITEMDATA, index, 0) as i64 == alert.id { SendMessageW(list, LB_SETCURSEL, index, 0); break; }
			index += 1;
		}
	}
}

pub fn route_message(window: HWND, message: &MSG) -> bool {
	unsafe {
		if window.is_null() || IsWindow(window) == 0 || (message.hwnd != window && IsChild(window, message.hwnd) == 0) { return false; }
		if message.message == WM_KEYDOWN && message.wParam == 27 { DestroyWindow(window); return true; }
		if message.message == WM_KEYDOWN && message.wParam == 13 && (message.hwnd == GetDlgItem(window, PRICE) || message.hwnd == GetDlgItem(window, COUNT)) {
			SendMessageW(window, WM_COMMAND, SAVE as WPARAM, 0);
			return true;
		}
		IsDialogMessageW(window, message) != 0
	}
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
	unsafe {
		if message == WM_COMMAND {
			let command: i32 = (wparam & 0xffff) as i32;
			match command {
				SAVE => {
					let result: Result<PriceAlert> = PriceAlert::new(&text(window, PRICE), &text(window, COUNT), None);
					match result {
						Ok(mut alert) => {
							alert.id = GetWindowLongPtrW(window, GWLP_USERDATA) as i64;
							alert.enabled = SendMessageW(GetDlgItem(window, ENABLED), BM_GETCHECK, 0, 0) != BST_UNCHECKED as LRESULT;
							alert.direction = match SendMessageW(GetDlgItem(window, DIRECTION), CB_GETCURSEL, 0, 0) { 1 => Direction::Up, 2 => Direction::Down, _ => Direction::Both };
							platform::submit_alert(alert);
						}
						Err(error) => { MessageBoxW(window, wide(&error.to_string()).as_ptr(), wide("请检查提醒设置").as_ptr(), MB_OK | MB_ICONWARNING); }
					}
				}
				NEW => { clear_editor(window); SetFocus(GetDlgItem(window, PRICE)); }
				TASKS if (wparam >> 16) as u32 == LBN_SELCHANGE => {
					if let Some(id) = selected_id(window) { platform::select_alert(id); }
				}
				DELETE | TOGGLE => {
					match selected_id(window) {
						Some(id) if command == TOGGLE => platform::toggle_alert(id),
						Some(id) => {
							if MessageBoxW(window, wide(&format!("确认删除提醒任务 #{id}？")).as_ptr(), wide("删除提醒任务").as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES { platform::delete_alert(id); }
						}
						None => { MessageBoxW(window, wide("请先在列表中选择任务").as_ptr(), wide("提醒任务").as_ptr(), MB_OK); }
					}
				}
				CANCEL => { DestroyWindow(window); }
				_ => return DefWindowProcW(window, message, wparam, lparam),
			}
			return 0;
		}
		match message {
			WM_CLOSE => { DestroyWindow(window); 0 }
			WM_DESTROY => { platform::settings_closed(window); 0 }
			_ => DefWindowProcW(window, message, wparam, lparam),
		}
	}
}
