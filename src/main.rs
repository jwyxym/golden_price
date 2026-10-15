#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod model;
mod storage;
mod drawing;
mod platform;
mod app;
mod alert;
mod settings;

use std::{fs::OpenOptions, io::Write, path::PathBuf};

use anyhow::Result;

fn main() {
	if let Err(error) = run() {
		log_error(&format!("{error:#}"));
		platform::show_error(&format!("启动失败：{error:#}"));
	}
}

fn run() -> Result<()> {
	platform::run()
}

pub fn log_error(message: &str) {
	let directory: PathBuf = storage::data_directory().unwrap_or_else(|_: anyhow::Error| PathBuf::from("."));
	if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(directory.join("golden-price.log")) {
		let _: std::io::Result<()> = writeln!(file, "{} {message}", chrono::Utc::now().to_rfc3339());
	}
	eprintln!("{message}");
}
