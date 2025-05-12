use ffmpeg::{format, media, util::frame::video::Video};
use ffmpeg_next as ffmpeg;
use float_ord::FloatOrd;
use plotters::{
    coord::ranged1d::{KeyPointHint, NoDefaultFormatting, ValueFormatter},
    prelude::*,
};
use std::{env, ops::Range, time::Instant};

// Contants from the SMPTE 2084 PQ spec
pub const ST2084_Y_MAX: f64 = 10000.0;
pub const ST2084_M1: f64 = 2610.0 / 16384.0;
pub const ST2084_M2: f64 = (2523.0 / 4096.0) * 128.0;
pub const ST2084_C1: f64 = 3424.0 / 4096.0;
pub const ST2084_C2: f64 = (2413.0 / 4096.0) * 32.0;
pub const ST2084_C3: f64 = (2392.0 / 4096.0) * 32.0;

const MAX_COLOUR: RGBColor = RGBColor(65, 105, 225);
const AVERAGE_COLOUR: RGBColor = RGBColor(75, 0, 130);
const MIN_COLOUR: RGBColor = BLACK;

fn pq_to_nits(pq: f64) -> f64 {
    let pq = pq.clamp(0.0, 1.0);

    // Inverse EOTF for PQ
    let v_p = pq.powf(1.0 / ST2084_M2);
    let n = ((v_p - ST2084_C1).max(0.0) / (ST2084_C2 - ST2084_C3 * v_p)).powf(1.0 / ST2084_M1);

    n * ST2084_Y_MAX
}

fn yuv420_10bit_to_pq(sample: u16) -> f64 {
    const YUV420_10BIT_MAX: f64 = 1023.0;
    let pq_code_value = (sample as f64).clamp(0.0, YUV420_10BIT_MAX);

    pq_code_value / 1023.0
}

pub fn nits_to_pq(nits: f64) -> f64 {
    let y = nits / ST2084_Y_MAX;

    ((ST2084_C1 + ST2084_C2 * y.powf(ST2084_M1)) / (1.0 + ST2084_C3 * y.powf(ST2084_M1)))
        .powf(ST2084_M2)
}

#[derive(Debug)]
struct FrameInfo {
    max: f64,
    min: f64,
    avg: f64,
}

impl FrameInfo {
    fn parse_frame(frame: &[u16]) -> Self {
        let mut sum = 0;
        let mut max = 0;
        let mut min = u16::MAX;

        for &sample in frame {
            sum += sample as usize;
            max = std::cmp::max(max, sample);
            min = std::cmp::min(min, sample);
        }

        let avg = (sum / frame.len()) as u16;

        FrameInfo {
            max: yuv420_10bit_to_pq(max),
            min: yuv420_10bit_to_pq(min),
            avg: yuv420_10bit_to_pq(avg),
        }
    }
}

fn main() -> Result<(), ffmpeg::Error> {
    ffmpeg::init()?;

    let input_path = env::args()
        .nth(1)
        .expect("Usage: pq_yuv_decoder <video_file>");

    let mut ictx = format::input(&input_path)?;
    let input = ictx
        .streams()
        .best(media::Type::Video)
        .ok_or(ffmpeg::Error::StreamNotFound)?;

    let num_frames = ictx
        .metadata()
        .get("NUMBER_OF_FRAMES")
        .and_then(|x| x.parse::<u64>().ok());

    let stream_index = input.index();
    let codec_params = input.parameters();
    let context_decoder = ffmpeg::codec::context::Context::from_parameters(codec_params)?;
    let mut decoder = context_decoder.decoder().video()?;

    println!("Input pixel format: {:?}", decoder.format());
    println!("Width x Height: {} x {}", decoder.width(), decoder.height());

    let mut decoded = Video::empty();
    let mut frame_count = 0;

    let mut results: Vec<FrameInfo> = Vec::new();
    let mut last = Instant::now();

    for (stream, packet) in ictx.packets() {
        if stream.index() == stream_index {
            decoder.send_packet(&packet)?;

            while decoder.receive_frame(&mut decoded).is_ok() {
                frame_count += 1;

                if frame_count % 100 == 0 {
                    let dur = Instant::now() - last;
                    if let Some(num_frames) = num_frames {
                        println!("{:02}%, last 100 took {:?}", frame_count / num_frames, dur);
                    } else {
                        let fps = 100.0 / dur.as_secs_f64();
                        let x = fps / 24.0;

                        println!("last 100 frames {:.02}fps ({:.03}x)", fps, x);
                    }
                    last = Instant::now();
                }

                // YUV420 10-bit (e.g., yuv420p10le)
                let y_plane = bytemuck::cast_slice::<u8, u16>(decoded.data(0));

                let frameinfo = FrameInfo::parse_frame(y_plane);

                results.push(frameinfo);
            }
        }
    }

    decoder.send_eof()?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        println!("Flushing frame {}", frame_count);
        frame_count += 1;
    }

    println!("Total decoded frames: {}", frame_count);

    plot(
        &results,
        std::path::Path::new("out.png"),
        "SMPTE 2084 PQ Measurements Plot",
    );

    Ok(())
}

pub struct PqCoord {}

impl Ranged for PqCoord {
    type FormatOption = NoDefaultFormatting;
    type ValueType = f64;

    fn map(&self, value: &f64, limit: (i32, i32)) -> i32 {
        let size = limit.1 - limit.0;
        (*value * size as f64) as i32 + limit.0
    }

    fn key_points<Hint: KeyPointHint>(&self, _hint: Hint) -> Vec<f64> {
        vec![
            nits_to_pq(0.01),
            nits_to_pq(0.1),
            nits_to_pq(0.5),
            nits_to_pq(1.0),
            nits_to_pq(2.5),
            nits_to_pq(5.0),
            nits_to_pq(10.0),
            nits_to_pq(25.0),
            nits_to_pq(50.0),
            nits_to_pq(100.0),
            nits_to_pq(200.0),
            nits_to_pq(400.0),
            nits_to_pq(600.0),
            nits_to_pq(1000.0),
            nits_to_pq(2000.0),
            nits_to_pq(4000.0),
            nits_to_pq(10000.0),
        ]
    }

    fn range(&self) -> Range<f64> {
        0_f64..10000.0_f64
    }
}
impl ValueFormatter<f64> for PqCoord {
    fn format_ext(&self, value: &f64) -> String {
        let nits = (pq_to_nits(*value) * 1000.0).round() / 1000.0;
        format!("{nits}")
    }
}

fn plot(results: &[FrameInfo], output: &std::path::Path, title: &str) {
    let root = BitMapBackend::new(output, (3000, 1200)).into_drawing_area();
    root.fill(&WHITE).unwrap();
    let root = root
        .margin(30, 30, 60, 60)
        .titled(title, ("sans-serif", 40))
        .unwrap();

    let x_spec = 0..results.len();

    let mut chart = ChartBuilder::on(&root)
        .x_label_area_size(60)
        .y_label_area_size(60)
        .margin_top(90)
        .build_cartesian_2d(x_spec, PqCoord {})
        .unwrap();

    chart
        .configure_mesh()
        .bold_line_style(BLACK.mix(0.10))
        .light_line_style(BLACK.mix(0.01))
        .label_style(("sans-serif", 22))
        .axis_desc_style(("sans-serif", 24))
        .x_desc("frames")
        .x_max_light_lines(1)
        .x_labels(24)
        .y_desc("nits (cd/m²)")
        .draw()
        .unwrap();

    let maxfall = pq_to_nits(results.iter().map(|x| FloatOrd(x.avg)).max().unwrap().0);
    let maxfall_avg = pq_to_nits(results.iter().map(|x| x.avg).sum::<f64>() / results.len() as f64);

    let avg_series_label = format!(
        "Average (MaxFALL: {:.2} nits, avg: {:.2} nits)",
        maxfall, maxfall_avg
    );

    let maxcll = pq_to_nits(results.iter().map(|x| FloatOrd(x.max)).max().unwrap().0);
    let maxcll_avg = pq_to_nits(results.iter().map(|x| x.max).sum::<f64>() / results.len() as f64);

    let max_series_label = format!(
        "Maximum (MaxCLL: {:.2} nits, avg: {:.2} nits)",
        maxcll, maxcll_avg,
    );

    let max_min = pq_to_nits(results.iter().map(|x| FloatOrd(x.min)).max().unwrap().0);
    let min_series_label = format!("Minimum (max: {:.6} nits)", max_min);

    let max_series = AreaSeries::new(
        (0..).zip(results).map(|(x, y)| (x, (y.max))),
        0.0,
        MAX_COLOUR.mix(0.25),
    )
    .border_style(MAX_COLOUR);
    let avg_series = AreaSeries::new(
        (0..).zip(results).map(|(x, y)| (x, (y.avg))),
        0.0,
        AVERAGE_COLOUR.mix(0.25),
    )
    .border_style(AVERAGE_COLOUR);
    let min_series = AreaSeries::new(
        (0..).zip(results).map(|(x, y)| (x, (y.min))),
        0.0,
        BLACK.mix(0.25),
    )
    .border_style(BLACK);

    chart
        .draw_series(max_series)
        .unwrap()
        .label(max_series_label)
        .legend(|(x, y)| {
            PathElement::new(
                vec![(x, y), (x + 20, y)],
                ShapeStyle {
                    color: MAX_COLOUR.to_rgba(),
                    filled: false,
                    stroke_width: 2,
                },
            )
        });

    chart
        .draw_series(avg_series)
        .unwrap()
        .label(avg_series_label)
        .legend(|(x, y)| {
            PathElement::new(
                vec![(x, y), (x + 20, y)],
                ShapeStyle {
                    color: AVERAGE_COLOUR.to_rgba(),
                    filled: false,
                    stroke_width: 2,
                },
            )
        });

    chart
        .draw_series(min_series)
        .unwrap()
        .label(min_series_label)
        .legend(|(x, y)| {
            PathElement::new(
                vec![(x, y), (x + 20, y)],
                ShapeStyle {
                    color: MIN_COLOUR.to_rgba(),
                    filled: false,
                    stroke_width: 2,
                },
            )
        });

    chart
        .configure_series_labels()
        .border_style(MIN_COLOUR)
        .position(SeriesLabelPosition::LowerLeft)
        .label_font(("sans-serif", 24))
        .background_style(WHITE)
        .draw()
        .unwrap();

    let chart_caption = format!("Frames: {}", results.len());

    let caption_style = ("sans-serif", 24).into_text_style(&root);
    root.draw_text(&chart_caption, &caption_style, (60, 10))
        .unwrap();
}
