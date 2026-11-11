use super::grid::ResultGrid;
use gpui::{App, Entity, Window, px};
use gpui_component::table::TableState;
use std::io::Write as _;
use std::time::Instant;

const SCROLL_STEP_PX: f32 = 2000.;
const MAX_FRAMES: usize = 600;

pub struct Bench {
    pub sql: String,
    pub connection: String,
    pub start: Instant,
}

impl Bench {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var("TABULARIS_BENCH_SQL").ok()?;
        let connection = std::env::var("TABULARIS_BENCH_CONNECTION").ok()?;
        match std::fs::read_to_string(&path) {
            Ok(sql) => Some(Self {
                sql,
                connection,
                start: Instant::now(),
            }),
            Err(error) => fail(&format!("{path}: {error}")),
        }
    }
}

pub fn fail(message: &str) -> ! {
    println!("BENCH error={}", message.replace('\n', " "));
    let _ = std::io::stdout().flush();
    std::process::exit(1)
}

/// Nearest-rank percentile of an ascending slice.
pub fn percentile(sorted: &[f64], pct: f64) -> f64 {
    let rank = (pct / 100. * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

struct Run {
    table: Entity<TableState<ResultGrid>>,
    rows: usize,
    all_loaded_ms: f64,
    first_paint_ms: f64,
    last_frame: Instant,
    intervals: Vec<f64>,
}

fn ms(from: Instant, to: Instant) -> f64 {
    to.duration_since(from).as_secs_f64() * 1000.
}

pub fn loaded(
    bench: Bench,
    rows: usize,
    table: Entity<TableState<ResultGrid>>,
    window: &mut Window,
) {
    let all_loaded_ms = ms(bench.start, Instant::now());
    window.on_next_frame(move |window, _| {
        let now = Instant::now();
        let run = Run {
            table,
            rows,
            all_loaded_ms,
            first_paint_ms: ms(bench.start, now),
            last_frame: now,
            intervals: Vec::new(),
        };
        window.on_next_frame(move |window, cx| scroll_step(run, window, cx));
    });
}

fn scroll_step(mut run: Run, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let interval = ms(run.last_frame, now);
    run.last_frame = now;
    run.intervals.push(interval);

    let handle = run
        .table
        .read(cx)
        .vertical_scroll_handle
        .0
        .borrow()
        .base_handle
        .clone();
    let max = handle.max_offset().y;
    let mut offset = handle.offset();
    offset.y = (offset.y - px(SCROLL_STEP_PX)).max(-max);
    handle.set_offset(offset);
    run.table.update(cx, |_, cx| cx.notify());

    if (max > px(0.) && -offset.y >= max) || run.intervals.len() >= MAX_FRAMES {
        report(run);
        cx.quit();
    } else {
        window.on_next_frame(move |window, cx| scroll_step(run, window, cx));
    }
}

fn report(run: Run) {
    let mut sorted = run.intervals;
    sorted.sort_by(f64::total_cmp);
    println!("BENCH all_loaded_ms={:.2}", run.all_loaded_ms);
    println!("BENCH first_paint_ms={:.2}", run.first_paint_ms);
    println!(
        "BENCH scroll_p50_ms={:.2} scroll_p99_ms={:.2} frames={}",
        percentile(&sorted, 50.),
        percentile(&sorted, 99.),
        sorted.len()
    );
    println!("BENCH rows={}", run.rows);
    let _ = std::io::stdout().flush();
}
