use std::collections::BTreeMap;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};

#[derive(Clone, Copy, Debug)]
pub struct Point {
	pub timestamp: f64,
	pub price: f64,
}

#[derive(Clone, Debug)]
pub struct Quote {
	pub point: Point,
	pub change: String,
	pub rate: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
	pub day: NaiveDate,
	pub start: i64,
	pub end: i64,
}

pub fn china_zone() -> FixedOffset {
	FixedOffset::east_opt(8 * 3600).expect("有效时区")
}

impl Session {
	pub fn current() -> Self {
		let now: DateTime<FixedOffset> = Utc::now().with_timezone(&china_zone());
		let day: NaiveDate = (now - Duration::hours(9) - Duration::minutes(10)).date_naive();
		Self::new(day)
	}

	pub fn new(day: NaiveDate) -> Self {
		let start: NaiveDateTime = day.and_hms_opt(9, 10, 0).expect("有效开盘时间");
		let end: NaiveDateTime = day.succ_opt().expect("有效日期").and_hms_opt(2, 0, 0).expect("有效收盘时间");
		Self {
			day,
			start: china_zone().from_local_datetime(&start).single().expect("固定时区").timestamp(),
			end: china_zone().from_local_datetime(&end).single().expect("固定时区").timestamp(),
		}
	}

	pub fn contains(self, point: Point) -> bool {
		point.timestamp.is_finite() && point.price.is_finite() && point.price > 0.0
			&& point.timestamp >= self.start as f64 && point.timestamp < self.end as f64
	}

	pub fn fraction(self, timestamp: f64) -> f32 {
		((timestamp - self.start as f64) / ((self.end - self.start) as f64 - 0.001)) as f32
	}
}

pub fn time_text(timestamp: f64) -> String {
	match DateTime::<Utc>::from_timestamp_millis((timestamp * 1000.0) as i64) {
		Some(date) => date.with_timezone(&china_zone()).format("%m-%d %H:%M:%S").to_string(),
		None => "时间无效".to_string(),
	}
}

pub fn merge_points(session: Session, history: &[Point], current: &[Point]) -> Vec<Point> {
	let mut points: BTreeMap<i64, Point> = BTreeMap::new();
	history.iter().chain(current.iter()).for_each(|point: &Point| {
		if session.contains(*point) {
			points.insert((point.timestamp * 1000.0).round() as i64, *point);
		}
	});
	points.into_values().collect()
}
