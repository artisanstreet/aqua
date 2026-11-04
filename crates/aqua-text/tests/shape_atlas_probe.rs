#![cfg(windows)]

use std::time::Instant;

use aqua_media::residency::{Residency, ResidencyConfig};
use aqua_text::residency::{GlyphResolver, RunStyle};
use aqua_text::{FontFamily, FontManager, FontWeight, StyledSpan, TextLayout, TextStyle, WrapMode};

const WORDS: [&str; 24] = [
    "the",
    "render",
    "thread",
    "owns",
    "atlas",
    "growth",
    "when",
    "streaming",
    "markdown",
    "blocks",
    "arrive",
    "token",
    "by",
    "token",
    "and",
    "shaping",
    "must",
    "stay",
    "incremental",
    "Because",
    "Quick",
    "Zebra",
    "jumped",
    "Hyphen-ated",
];

fn sentence(seed: usize, words: usize) -> String {
    (0..words)
        .map(|i| WORDS[(seed * 7 + i * 13 + i * i) % WORDS.len()])
        .collect::<Vec<_>>()
        .join(" ")
}

fn spans_for(seed: usize, words: usize) -> Vec<StyledSpan> {
    let body = TextStyle::default();
    let bold = TextStyle {
        weight: FontWeight::BOLD,
        ..body.clone()
    };
    let code = TextStyle {
        families: vec![FontFamily::Monospace],
        size_px: 14.0,
        ..body.clone()
    };
    vec![
        StyledSpan::new(sentence(seed, words) + " ", body.clone()),
        StyledSpan::new(sentence(seed + 1, 3) + " ", bold),
        StyledSpan::new(sentence(seed + 2, words / 2) + " ", body),
        StyledSpan::new(format!("fn_{seed}(x)"), code),
    ]
}

fn layout_of(spans: Vec<StyledSpan>, width: f32) -> TextLayout {
    let mut layout = TextLayout::new();
    layout.set_spans(spans);
    layout.set_wrap(WrapMode::Word);
    layout.set_max_width(Some(width));
    layout
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[((samples.len() - 1) as f64 * p) as usize]
}

#[test]
#[ignore]
fn shaping_and_atlas_costs() {
    let mut fonts = FontManager::new();
    let blocks = 400;
    let warm = layout_of(spans_for(9999, 20), 700.0);
    let mut warm = warm;
    warm.layout(&mut fonts);

    let start = Instant::now();
    let mut lays: Vec<_> = (0..blocks)
        .map(|n| layout_of(spans_for(n, 40), 700.0))
        .collect();
    let laids: Vec<_> = lays.iter_mut().map(|l| l.layout(&mut fonts)).collect();
    println!(
        "cold_shape blocks={blocks} total_ms={:.1} per_block_us={:.0}",
        start.elapsed().as_secs_f64() * 1e3,
        start.elapsed().as_secs_f64() * 1e6 / blocks as f64
    );

    let start = Instant::now();
    let mut rewidth: Vec<_> = (0..blocks)
        .map(|n| layout_of(spans_for(n, 40), 640.0))
        .collect();
    for l in rewidth.iter_mut() {
        l.layout(&mut fonts);
    }
    println!(
        "fresh_layout_other_width total_ms={:.1} per_block_us={:.0}",
        start.elapsed().as_secs_f64() * 1e3,
        start.elapsed().as_secs_f64() * 1e6 / blocks as f64
    );

    let start = Instant::now();
    let mut again: Vec<_> = (0..blocks)
        .map(|n| layout_of(spans_for(n, 40), 700.0))
        .collect();
    for l in again.iter_mut() {
        l.layout(&mut fonts);
    }
    println!(
        "fresh_layout_same_input total_ms={:.1} per_block_us={:.0}",
        start.elapsed().as_secs_f64() * 1e3,
        start.elapsed().as_secs_f64() * 1e6 / blocks as f64
    );

    let mut residency = Residency::new(ResidencyConfig::default());
    let mut resolver = GlyphResolver::new();
    let style = RunStyle {
        color: aqua_core::Oklch::BLACK,
        opacity: 1.0,
        clip: None,
    };
    let screen = 12;
    let mut costs = Vec::new();
    let mut pages_before = residency.stats().masks.pages;
    let mut growth_frames = 0;
    let mut uploads = 0usize;
    for first in 0..blocks - screen {
        residency.begin_frame();
        let start = Instant::now();
        for (row, laid) in laids[first..first + screen].iter().enumerate() {
            resolver
                .lower(
                    &mut residency,
                    &mut fonts,
                    laid,
                    [10.0, 20.0 + row as f32 * 90.0],
                    style,
                )
                .unwrap();
        }
        let updates = residency.finish_frame();
        costs.push(start.elapsed().as_secs_f64() * 1e3);
        uploads += updates.uploads.len();
        let pages = residency.stats().masks.pages;
        if pages != pages_before {
            growth_frames += 1;
            pages_before = pages;
        }
    }
    let stats = residency.stats();
    println!(
        "scroll_lower frames={} p50={:.3} p99={:.3} max={:.3} pages={} growth_frames={} uploads={} inserts={} entries={}",
        costs.len(),
        percentile(&mut costs.clone(), 0.5),
        percentile(&mut costs.clone(), 0.99),
        percentile(&mut costs, 1.0),
        stats.masks.pages, growth_frames, uploads, stats.masks.inserts, stats.masks.entries
    );
    let mut steady = Vec::new();
    for first in 0..blocks - screen {
        residency.begin_frame();
        let start = Instant::now();
        for (row, laid) in laids[first..first + screen].iter().enumerate() {
            resolver
                .lower(
                    &mut residency,
                    &mut fonts,
                    laid,
                    [10.0, 20.0 + row as f32 * 90.0],
                    style,
                )
                .unwrap();
        }
        let updates = residency.finish_frame();
        assert!(updates.uploads.is_empty());
        steady.push(start.elapsed().as_secs_f64() * 1e3);
    }
    println!(
        "scroll_lower_second_pass p50={:.3} p99={:.3}",
        percentile(&mut steady.clone(), 0.5),
        percentile(&mut steady, 1.0)
    );

    let mut cache = aqua_text::slug::SlugCache::new();
    let mut store = aqua_text::slug::SlugStore::new();
    let mut slug_costs = Vec::new();
    let mut creates = 0;
    let mut upload_bytes = 0usize;
    let mut growth_frames_slug = Vec::new();
    for first in 0..blocks - screen {
        let start = Instant::now();
        for (row, laid) in laids[first..first + screen].iter().enumerate() {
            let _ = row;
            for line in &laid.lines {
                for glyph in &line.glyphs {
                    let key = aqua_text::raster::raster_key(glyph, 0);
                    store.lookup(&mut cache, &mut fonts, &key);
                }
            }
        }
        let batch = store.uploads();
        for texture in &batch.textures {
            match &texture.op {
                aqua_protocol::TextureOp::Create { .. } => {
                    creates += 1;
                    growth_frames_slug.push(first);
                }
                aqua_protocol::TextureOp::Write { data, .. } => upload_bytes += data.len(),
                _ => {}
            }
        }
        slug_costs.push(start.elapsed().as_secs_f64() * 1e3);
    }
    println!(
        "slug_scroll frames={} p50={:.3} p99={:.3} max={:.3} creates={} at_frames={:?} upload_bytes={} store_bytes={} entries={}",
        slug_costs.len(),
        percentile(&mut slug_costs.clone(), 0.5),
        percentile(&mut slug_costs.clone(), 0.99),
        percentile(&mut slug_costs, 1.0),
        creates,
        growth_frames_slug,
        upload_bytes,
        store.resident_bytes(),
        store.len()
    );

    let tokens: Vec<String> = sentence(5, 220)
        .split(' ')
        .map(|w| format!("{w} "))
        .collect();
    let mut text = String::new();
    let mut per_token = Vec::new();
    let body = TextStyle::default();
    for token in &tokens {
        text.push_str(token);
        let mut spans = vec![StyledSpan::new(text.clone(), body.clone())];
        spans.push(StyledSpan::new("tail", body.clone()));
        let start = Instant::now();
        let mut layout = layout_of(spans, 700.0);
        layout.layout(&mut fonts);
        per_token.push(start.elapsed().as_secs_f64() * 1e3);
    }
    println!(
        "stream_token_fresh_layout tokens={} chars={} p50={:.3} p99={:.3} max={:.3}",
        tokens.len(),
        text.len(),
        percentile(&mut per_token.clone(), 0.5),
        percentile(&mut per_token.clone(), 0.99),
        percentile(&mut per_token, 1.0)
    );

    let code = TextStyle {
        families: vec![FontFamily::Monospace],
        size_px: 13.0,
        ..TextStyle::default()
    };
    let lines: Vec<String> = (0..120)
        .map(|n| format!("    let value_{n} = compute(&state, {n}).unwrap_or_default();"))
        .collect();
    let mut streamed = lines.join("\n");
    streamed.push('\n');
    let mut per_code = Vec::new();
    for token in sentence(3, 60).split(' ') {
        streamed.push_str(token);
        streamed.push(' ');
        let start = Instant::now();
        let mut layout = TextLayout::new();
        layout.set_spans(vec![StyledSpan::new(streamed.clone(), code.clone())]);
        layout.set_line_mode(aqua_text::LineMode::Multi);
        layout.set_wrap(WrapMode::None);
        layout.layout(&mut fonts);
        per_code.push(start.elapsed().as_secs_f64() * 1e3);
    }
    println!(
        "stream_code_block_120_lines tokens={} p50={:.3} p99={:.3} max={:.3}",
        per_code.len(),
        percentile(&mut per_code.clone(), 0.5),
        percentile(&mut per_code.clone(), 0.99),
        percentile(&mut per_code, 1.0)
    );
    println!("shape_cache {:?}", fonts.shape_cache_stats());
}
