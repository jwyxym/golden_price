use anyhow::{Context, Result, bail};

use crate::model::Point;

#[derive(Clone, Copy, Debug)]
pub enum Direction {
	Both,
	Up,
	Down,
}

impl Direction {
	pub fn key(self) -> &'static str {
		match self { Self::Both => "both", Self::Up => "up", Self::Down => "down" }
	}

	pub fn label(self) -> &'static str {
		match self { Self::Both => "双向提醒", Self::Up => "涨到提醒", Self::Down => "跌到提醒" }
	}

	pub fn from_key(key: &str) -> Result<Self> {
		match key { "both" => Ok(Self::Both), "up" => Ok(Self::Up), "down" => Ok(Self::Down), _ => bail!("未知提醒方向") }
	}
}

#[derive(Clone, Debug)]
pub struct PriceAlert {
	pub id: i64,
	pub direction: Direction,
	pub enabled: bool,
	pub target_cents: i64,
	pub limit: u32,
	pub remaining: u32,
	pub last_price_cents: Option<i64>,
	pub last_timestamp: Option<f64>,
}

impl PriceAlert {
	pub fn new(price: &str, count: &str, latest: Option<Point>) -> Result<Self> {
		let text: &str = price.trim();
		let parts: Vec<&str> = text.split('.').collect();
		if parts.len() > 2 || parts[0].is_empty() || !parts[0].bytes().all(|byte: u8| byte.is_ascii_digit()) {
			bail!("请输入大于零的提醒价格，最多两位小数");
		}
		let fraction: &str = parts.get(1).copied().unwrap_or("");
		if fraction.len() > 2 || !fraction.bytes().all(|byte: u8| byte.is_ascii_digit()) {
			bail!("提醒价格最多保留两位小数");
		}
		let integer: i64 = parts[0].parse().context("提醒价格过大")?;
		let cents: i64 = if fraction.is_empty() { 0 } else { fraction.parse::<i64>()? * if fraction.len() == 1 { 10 } else { 1 } };
		let target_cents: i64 = integer.checked_mul(100).and_then(|value: i64| value.checked_add(cents)).context("提醒价格过大")?;
		let limit: u32 = count.trim().parse().context("通知次数必须为正整数")?;
		if target_cents <= 0 || limit == 0 { bail!("提醒价格和通知次数都必须大于零"); }
		let last_price_cents: Option<i64> = latest.map(|point: Point| (point.price * 100.0).round() as i64);
		Ok(Self { id: 0, direction: Direction::Both, enabled: true, target_cents, limit, remaining: limit, last_price_cents, last_timestamp: latest.map(|point: Point| point.timestamp) })
	}

	pub fn target_text(&self) -> String {
		format!("{}.{:02}", self.target_cents / 100, self.target_cents % 100)
	}

	pub fn observe(&mut self, point: Point) -> bool {
		if !self.enabled || self.remaining == 0 || !point.price.is_finite() || !point.timestamp.is_finite() || point.price <= 0.0
			|| self.last_timestamp.is_some_and(|timestamp: f64| point.timestamp <= timestamp) {
			return false;
		}
		let current: i64 = (point.price * 100.0).round() as i64;
		let hit: bool = match self.last_price_cents {
			Some(previous) => {
				let up: bool = previous < self.target_cents && current >= self.target_cents;
				let down: bool = previous > self.target_cents && current <= self.target_cents;
				match self.direction { Direction::Both => up || down, Direction::Up => up, Direction::Down => down }
			}
			// 首条报价只建立基准，不能据此判断涨跌方向。
			None => false,
		};
		self.last_price_cents = Some(current);
		self.last_timestamp = Some(point.timestamp);
		hit && self.remaining > 0
	}
}
