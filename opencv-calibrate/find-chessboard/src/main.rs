use image::GenericImageView;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(about = "find chessboard corners in an input image")]
struct CliArgs {
    /// Pattern width (the number of horizontal corners in the image)
    pattern_width: usize,
    /// Pattern height (the number of vertical corners in the image)
    pattern_height: usize,
    /// Input image
    #[structopt(parse(from_os_str))]
    image: std::path::PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = CliArgs::from_args();

    let img = image::open(&args.image)?;

    let (w, h) = img.dimensions();
    let rgb = img.to_rgb8().into_raw();

    let start = std::time::Instant::now();
    let corners = opencv_calibrate::find_chessboard_corners(
        &rgb,
        w,
        h,
        args.pattern_width,
        args.pattern_height,
    )?;
    let duration = start.elapsed();
    let seconds = duration.as_nanos() as f64 / 1e9;
    println!("# processing duration: {}", seconds);
    if let Some(corners) = corners {
        let corners_yaml = serde_yaml::to_string(&corners)?;
        println!("{}", corners_yaml);
    }
    Ok(())
}
