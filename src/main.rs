use ffmpeg::{decoder, format, media, util::frame::video::Video};
use ffmpeg_next as ffmpeg;
use plotters::prelude::*;
use std::{
    cmp::{max, min},
    env,
    time::Instant,
};

use rayon::prelude::*;

/// Converts a 10-bit PQ (Perceptual Quantizer) code value to nits (cd/m²)
///
/// The PQ function is defined in BT.2100 and SMPTE ST 2084
///
/// # Arguments
/// * `pq_code_value` - A 10-bit PQ code value (0-1023) as a u16
///
/// # Returns
/// * The equivalent brightness in nits (cd/m²)
pub fn pq_to_nits(pq_code_value: u16) -> f32 {
    // Clamp input to valid 10-bit range [0, 1023]
    let pq_code_value = pq_code_value.min(1023);

    // Convert 10-bit code value to normalized [0.0, 1.0] range
    let pq_value = pq_code_value as f32 / 1023.0;

    // Constants defined in SMPTE ST 2084
    const M1: f32 = 0.1593017578125; // 2610.0 / 4096.0 / 4.0
    const M2: f32 = 78.84375; // 2523.0 / 4096.0 * 128.0
    const C1: f32 = 0.8359375; // 3424.0 / 4096.0 or 107.0 / 128.0
    const C2: f32 = 18.8515625; // 2413.0 / 4096.0 * 32.0
    const C3: f32 = 18.6875; // 2392.0 / 4096.0 * 32.0

    // Maximum brightness in nits that can be represented
    const MAX_NITS: f32 = 10000.0;

    // Inverse EOTF for PQ
    let v_p = pq_value.powf(1.0 / M2);
    let n = ((v_p - C1).max(0.0) / (C2 - C3 * v_p)).powf(1.0 / M1);

    // Convert to nits
    n * MAX_NITS
}

#[derive(Debug)]
struct FrameInfo {
    max: u16,
    min: u16,
    avg: u16,
}

impl FrameInfo {
    fn parse_frame(frame: &[u16]) -> Self {
        // Step 1: Parallel processing of the frame to find min, max, and sum
        let (min_val, max_val, sum_val) = frame
            .par_chunks(frame.len() / num_cpus::get()) // Split the frame into chunks
            .map(|chunk| {
                let (chunk_min, chunk_max, chunk_sum) = chunk.iter().fold(
                    (u16::MAX, u16::MIN, 0u16),
                    |(min_acc, max_acc, sum_acc), &value| {
                        (min(min_acc, value), max(max_acc, value), sum_acc + value)
                    },
                );
                (chunk_min, chunk_max, chunk_sum)
            })
            .reduce(
                || (u16::MAX, u16::MIN, 0u16), // Identity for min, max, and sum
                |(min_acc, max_acc, sum_acc), (chunk_min, chunk_max, chunk_sum)| {
                    // Combine results from chunks
                    (
                        min(min_acc, chunk_min),
                        max(max_acc, chunk_max),
                        sum_acc + chunk_sum,
                    )
                },
            );

        // Step 2: Calculate average by dividing the total sum by the total number of pixels
        let avg_val = sum_val / frame.len() as u16;

        // Step 3: Return the calculated values in a FrameInfo struct
        FrameInfo {
            max: max_val,
            min: min_val,
            avg: avg_val,
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

    let mut results: Vec<(FrameInfo)> = Vec::new();
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

                let maxcll = *y_plane.iter().max().unwrap();
                let maxfall =
                    (y_plane.iter().map(|x| *x as usize).sum::<usize>() / y_plane.len()) as u16;

                assert!(maxfall <= maxcll);

                let frameinfo = FrameInfo::parse_frame(y_plane);

                results.push(frameinfo);
            }
        }
    }

    // Flush decoder
    decoder.send_eof()?;
    while decoder.receive_frame(&mut decoded).is_ok() {
        println!("Flushing frame {}", frame_count);
        frame_count += 1;
    }

    println!("Total decoded frames: {}", frame_count);

    plot(&results);

    for result in results {
        dbg!(pq_to_nits(result.max));
    }

    Ok(())
}

fn plot(results: &[FrameInfo]) {
    let root_area = BitMapBackend::new("out.png", (1920, 1080)).into_drawing_area();
    root_area.fill(&WHITE).unwrap();

    let data_max = (pq_to_nits(results.iter().map(|x| x.max).max().unwrap()) as f64 * 1.1) as i32;

    let mut ctx = ChartBuilder::on(&root_area)
        .set_label_area_size(LabelAreaPosition::Left, 40)
        .set_label_area_size(LabelAreaPosition::Bottom, 40)
        .caption("Scatter Demo", ("sans-serif", 40))
        .build_cartesian_2d(0..results.len(), 0..data_max)
        .unwrap();

    ctx.configure_mesh().draw().unwrap();

    ctx.draw_series(
        AreaSeries::new(
            (0..).zip(results.iter().map(|x| pq_to_nits(x.max) as i32)), // The data iter
            0,                                                           // Baseline
            RED.mix(0.2),                                                // Make the series opac
        )
        .border_style(RED), // Make a brighter border
    )
    .unwrap();
    ctx.draw_series(
        AreaSeries::new(
            (0..).zip(results.iter().map(|x| pq_to_nits(x.avg) as i32)), // The data iter
            0,                                                           // Baseline
            BLUE.mix(0.2),                                               // Make the series opac
        )
        .border_style(BLUE), // Make a brighter border
    )
    .unwrap();
    ctx.draw_series(
        AreaSeries::new(
            (0..).zip(results.iter().map(|x| pq_to_nits(x.min) as i32)), // The data iter
            0,                                                           // Baseline
            GREEN.mix(0.2),                                              // Make the series opac
        )
        .border_style(GREEN), // Make a brighter border
    )
    .unwrap();
}
