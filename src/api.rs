use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{NaiveDateTime, TimeZone};
use reqwest::blocking::{Client, Response};
use serde_json::Value;

use crate::model::{Point, Quote, Session, china_zone};

const BASE: &str = "https://ms.jr.jd.com/gw/generic/hj/h5/m";

pub fn client() -> Result<Client> {
	Client::builder().timeout(Duration::from_secs(10)).build().context("创建行情客户端失败")
}

fn fetch(client: &Client, endpoint: &str) -> Result<Value> {
	let response: Response = client.post(format!("{BASE}/{endpoint}"))
		.query(&[("reqData", "{}")]).body(Vec::<u8>::new()).send()?.error_for_status()?;
	let result: Value = response.json()?;
	if result["success"].as_bool() != Some(true)
		|| result["resultData"]["status"].as_str().is_some_and(|status: &str| status != "SUCCESS") {
		bail!("行情接口返回失败：{}", result["resultMsg"]);
	}
	result["resultData"].get("datas").cloned().context("响应缺少 datas")
}

fn number(value: &Value) -> Result<f64> {
	let number: f64 = match value.as_f64() {
		Some(value) => value,
		None => value.as_str().context("数值格式错误")?.parse::<f64>()?,
	};
	if !number.is_finite() {
		bail!("报价包含无效数值");
	}
	Ok(number)
}

pub fn latest(client: &Client) -> Result<Quote> {
	let data: Value = fetch(client, "latestPrice")?;
	let price: f64 = number(&data["price"])?;
	if price <= 0.0 {
		bail!("报价必须大于零");
	}
	Ok(Quote {
		point: Point { timestamp: number(&data["time"])? / 1000.0, price },
		change: data["upAndDownAmt"].as_str().unwrap_or("--").to_string(),
		rate: data["upAndDownRate"].as_str().unwrap_or("--").to_string(),
	})
}

pub fn history(client: &Client, session: Session) -> Result<Vec<Point>> {
	let data: Value = fetch(client, "todayPrices")?;
	let rows: &Vec<Value> = data.as_array().context("分时数据不是数组")?;
	let mut points: Vec<Point> = Vec::with_capacity(rows.len());
	rows.iter().try_for_each(|row: &Value| -> Result<()> {
		// 最低/最高点可能带第三项 lowKey/highKey，只读取前两项。
		let text: &str = row["value"][0].as_str().context("分时记录缺少时间")?;
		let date: NaiveDateTime = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")?;
		let point: Point = Point {
			timestamp: china_zone().from_local_datetime(&date).single().context("分时时间无效")?.timestamp() as f64,
			price: number(&row["value"][1])?,
		};
		if session.contains(point) {
			points.push(point);
		}
		Ok(())
	})?;
	Ok(points)
}
