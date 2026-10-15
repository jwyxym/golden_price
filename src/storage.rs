use std::{fs, path::{Path, PathBuf}, time::Duration};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, Row, Rows, Statement, Transaction, params};

use crate::{alert::{Direction, PriceAlert}, model::{Point, Session}};

pub fn data_directory() -> Result<PathBuf> {
	if cfg!(debug_assertions) {
		Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
	} else {
		let executable: PathBuf = std::env::current_exe()?;
		Ok(executable.parent().context("找不到程序目录")?.to_path_buf())
	}
}

pub struct Store {
	connection: Connection,
}

impl Store {
	pub fn open(directory: &Path) -> Result<Self> {
		fs::create_dir_all(directory)?;
		let path: PathBuf = directory.join("gold-history.sqlite3");
		let original: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("gold-history.sqlite3");
		if !path.exists() && original.is_file() {
			// 使用 SQLite 在线备份读取快照，不直接复制正在使用的数据库文件。
			let source: Connection = Connection::open_with_flags(&original, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
			source.backup("main", &path, None)?;
		}
		let connection: Connection = Connection::open(path)?;
		connection.busy_timeout(Duration::from_secs(2))?;
		connection.execute_batch("CREATE TABLE IF NOT EXISTS price_points (
			trading_day TEXT NOT NULL, timestamp REAL NOT NULL,
			price REAL NOT NULL CHECK(price > 0), PRIMARY KEY(trading_day, timestamp)
		);
		CREATE TABLE IF NOT EXISTS price_alert (
			id INTEGER PRIMARY KEY CHECK(id = 1),
			target_cents INTEGER NOT NULL CHECK(target_cents > 0),
			notification_limit INTEGER NOT NULL CHECK(notification_limit > 0),
			remaining INTEGER NOT NULL CHECK(remaining >= 0 AND remaining <= notification_limit),
			last_price_cents INTEGER,
			last_timestamp REAL
		);
		CREATE TABLE IF NOT EXISTS price_alert_tasks (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			target_cents INTEGER NOT NULL CHECK(target_cents > 0),
			notification_limit INTEGER NOT NULL CHECK(notification_limit > 0),
			remaining INTEGER NOT NULL CHECK(remaining >= 0 AND remaining <= notification_limit),
			last_price_cents INTEGER, last_timestamp REAL,
			direction TEXT NOT NULL CHECK(direction IN ('both', 'up', 'down')),
			enabled INTEGER NOT NULL CHECK(enabled IN (0, 1))
		);
		CREATE TABLE IF NOT EXISTS app_migrations (name TEXT PRIMARY KEY);
		BEGIN IMMEDIATE;
		INSERT INTO price_alert_tasks (id, target_cents, notification_limit, remaining, last_price_cents, last_timestamp, direction, enabled)
		SELECT id, target_cents, notification_limit, remaining, last_price_cents, last_timestamp, 'both', 1 FROM price_alert
		WHERE NOT EXISTS (SELECT 1 FROM app_migrations WHERE name = 'multiple_alerts');
		INSERT OR IGNORE INTO app_migrations VALUES ('multiple_alerts');
		COMMIT;
		")?;
		Ok(Self { connection })
	}

	pub fn load(&self, session: Session) -> Result<Vec<Point>> {
		let mut statement: Statement<'_> = self.connection.prepare(
			"SELECT timestamp, price FROM price_points WHERE trading_day = ?1 AND timestamp >= ?2 AND timestamp < ?3 ORDER BY timestamp"
		)?;
		let points: Vec<Point> = statement.query_map(params![session.day.to_string(), session.start, session.end], |row: &Row<'_>| {
			Ok(Point { timestamp: row.get(0)?, price: row.get(1)? })
		})?.collect::<rusqlite::Result<Vec<Point>>>()?;
		Ok(points.into_iter().filter(|point: &Point| session.contains(*point)).collect())
	}

	pub fn load_alerts(&self) -> Result<Vec<PriceAlert>> {
		let mut statement: Statement<'_> = self.connection.prepare(
			"SELECT id, target_cents, notification_limit, remaining, last_price_cents, last_timestamp, direction, enabled FROM price_alert_tasks ORDER BY id"
		)?;
		let mut alerts: Vec<PriceAlert> = Vec::new();
		let mut rows: Rows<'_> = statement.query([])?;
		while let Some(row) = rows.next()? {
			let key: String = row.get(6)?;
			alerts.push(PriceAlert {
				id: row.get(0)?, target_cents: row.get(1)?, limit: row.get(2)?, remaining: row.get(3)?,
				last_price_cents: row.get(4)?, last_timestamp: row.get(5)?, direction: Direction::from_key(&key)?, enabled: row.get(7)?,
			});
		}
		Ok(alerts)
	}

	pub fn upsert_alert(&self, alert: &PriceAlert) -> Result<i64> {
		self.connection.execute(
			"INSERT INTO price_alert_tasks VALUES (NULLIF(?1, 0), ?2, ?3, ?4, ?5, ?6, ?7, ?8)
			ON CONFLICT(id) DO UPDATE SET target_cents=excluded.target_cents, notification_limit=excluded.notification_limit,
			remaining=excluded.remaining, last_price_cents=excluded.last_price_cents, last_timestamp=excluded.last_timestamp,
			direction=excluded.direction, enabled=excluded.enabled",
			params![alert.id, alert.target_cents, alert.limit, alert.remaining, alert.last_price_cents, alert.last_timestamp, alert.direction.key(), alert.enabled],
		)?;
		Ok(if alert.id == 0 { self.connection.last_insert_rowid() } else { alert.id })
	}

	pub fn save_alerts(&mut self, alerts: &[PriceAlert]) -> Result<()> {
		let transaction: Transaction<'_> = self.connection.transaction()?;
		{
			let mut statement: Statement<'_> = transaction.prepare(
				"UPDATE price_alert_tasks SET remaining=?2, last_price_cents=?3, last_timestamp=?4 WHERE id=?1"
			)?;
			alerts.iter().try_for_each(|alert: &PriceAlert| -> rusqlite::Result<()> {
				statement.execute(params![alert.id, alert.remaining, alert.last_price_cents, alert.last_timestamp])?;
				Ok(())
			})?;
		}
		transaction.commit()?;
		Ok(())
	}

	pub fn delete_alert(&self, id: i64) -> Result<()> {
		self.connection.execute("DELETE FROM price_alert_tasks WHERE id = ?1", [id])?;
		Ok(())
	}

	pub fn save(&mut self, session: Session, points: &[Point]) -> Result<()> {
		let transaction: Transaction<'_> = self.connection.transaction()?;
		let day: String = session.day.to_string();
		transaction.execute("DELETE FROM price_points WHERE trading_day <> ?1", [&day])?;
		{
			let mut statement: Statement<'_> = transaction.prepare(
				"INSERT INTO price_points VALUES (?1, ?2, ?3) ON CONFLICT(trading_day, timestamp) DO UPDATE SET price=excluded.price"
			)?;
			points.iter().try_for_each(|point: &Point| -> rusqlite::Result<()> {
				if session.contains(*point) {
					statement.execute(params![day, point.timestamp, point.price])?;
				}
				Ok(())
			})?;
		}
		transaction.commit()?;
		Ok(())
	}
}
