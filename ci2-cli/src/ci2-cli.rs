#[macro_use]
extern crate log;
extern crate env_logger;

extern crate failure;
extern crate structopt;

extern crate machine_vision_formats;
extern crate timestamped_frame;

extern crate ci2;
#[cfg(feature = "backend_aravis")]
extern crate ci2_aravis as backend;
#[cfg(feature = "backend_dc1394")]
extern crate ci2_dc1394 as backend;
#[cfg(feature = "backend_flycap2")]
extern crate ci2_flycap2 as backend;
#[cfg(feature = "backend_pyloncxx")]
extern crate ci2_pyloncxx as backend;
extern crate machine_vision_formats as formats;

use structopt::StructOpt;

use ci2::{Camera, CameraModule};
use timestamped_frame::HostTimeData;

#[derive(Debug, StructOpt)]
struct Record {
    /// set the recording duration in number of frames. 0 means infinite.
    #[structopt(
        short = "n",
        long = "num-frames",
        name = "NUM_FRAMES",
        default_value = "10"
    )]
    num_frames: usize,

    /// specify the name of the camera to use
    #[structopt(short = "c", long = "camera-name", name = "CAMERA_NAME")]
    camera_name: Option<String>,
}

#[derive(Debug, StructOpt)]
#[structopt(name = "ci2", about = "camera utilities")]
enum Command {
    /// record frames
    #[structopt(name = "record")]
    Record(Record),

    /// list cameras
    #[structopt(name = "list")]
    List,
}

fn list(mymod: backend::WrappedModule) -> ci2::Result<()> {
    let infos = mymod.camera_infos()?;
    for info in infos.iter() {
        println!("{}", info.name());
    }
    Ok(())
}

fn record(mut mymod: backend::WrappedModule, recargs: Record) -> ci2::Result<()> {
    let name = if let Some(camera_name) = recargs.camera_name {
        camera_name
    } else {
        let infos = mymod.camera_infos()?;
        if infos.len() == 0 {
            return Err("no cameras detected".into());
        }
        infos[0].name().to_string()
    };

    let mut cam = mymod.camera(&name)?;

    info!("got camera");
    cam.acquisition_start()?;
    let mut count = 0;
    loop {
        if recargs.num_frames != 0 && count >= recargs.num_frames {
            break;
        }
        count += 1;

        match cam.next_frame() {
            Ok(frame) => {
                info!("got frame {}: {:?}", frame.host_framenumber(), frame);
            }
            Err(ci2::Error::SingleFrameError(s)) => {
                error!("SingleFrameError({})", s);
            }
            Err(e) => {
                return Err(e);
            }
        }
    }
    cam.acquisition_stop()?;

    Ok(())
}

fn main() -> Result<(), failure::Error> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "ci2=info,error");
    }

    env_logger::init();
    let opt = Command::from_args();

    let mymod = backend::new_module()?;

    match opt {
        Command::Record(recargs) => record(mymod, recargs)?,
        Command::List => list(mymod)?,
    };

    Ok(())
}
