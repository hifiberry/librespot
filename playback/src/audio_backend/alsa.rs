use super::{Open, Sink};
use alsa::device_name::HintIter;
use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, Error, ValueOr};
use std::env;
use std::ffi::CString;
use std::io;
use std::process::exit;
use std::process::Command;

pub struct AlsaSink(Option<PCM>, String);

fn list_outputs() {
    for t in &["pcm", "ctl", "hwdep"] {
        println!("{} devices:", t);
        let i = HintIter::new(None, &*CString::new(*t).unwrap()).unwrap();
        for a in i {
            if let Some(Direction::Playback) = a.direction {
                // mimic aplay -L
                println!(
                    "{}\n\t{}\n",
                    a.name.unwrap(),
                    a.desc.unwrap().replace("\n", "\n\t")
                );
            }
        }
    }
}

fn open_device(dev_name: &str) -> Result<PCM, Box<Error>> {
    // Stop other players
    let output = Command::new("/opt/hifiberry/bin/pause-all").arg("vollibrespot").output().expect("failed to execute process");
    if !output.status.success() {
       error!("couldn't stop other players using pause-all");
    }
    
    let pcm = PCM::new(dev_name, Direction::Playback, false)?;
    // http://www.linuxjournal.com/article/6735?page=0,1#N0x19ab2890.0x19ba78d8
    // latency = period_size * periods / (rate * bytes_per_frame)
    // For 16 Bit stereo data, one frame has a length of four bytes.
    // 500ms  = buffer_size / (44100 * 4)
    // buffer_size = 0.5 * 44100 = 22050 frames
    {
        // Set hardware parameters: 44100 Hz / Stereo / 16 bit
        let hwp = HwParams::any(&pcm)?;

        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_format(Format::s16())?;
        hwp.set_rate(44100, ValueOr::Nearest)?;
        hwp.set_channels(2)?;
        hwp.set_buffer_size_near(22052)?; // ~ 0.5s latency
        hwp.set_period_size_near(5513, ValueOr::Greater)?;
        if env::var("LIBRESPOT_RATE_RESAMPLE").is_ok() {
            debug!("Allowing resampling, and setting period size: 1024");
            hwp.set_rate_resample(true)?;
            hwp.set_period_size_near(1024, ValueOr::Nearest)?;
        }
        pcm.hw_params(&hwp)?;

        let swp = pcm.sw_params_current()?;
        swp.set_start_threshold(hwp.get_buffer_size()? - hwp.get_period_size()?)?;
        pcm.sw_params(&swp)?;
    }

    // Additional software paramters + check
    if env::var("LIBRESPOT_DEBUG").is_ok() {
        let hwp = pcm.hw_params_current()?;
        let swp = pcm.sw_params_current()?;
        let (bufsize, periodsize) = (hwp.get_buffer_size()?, hwp.get_period_size()?);
        let periods = hwp.get_periods()?;
        info!(
            "periods: {:?} buffer_size: {:?} period_size {:?}",
            periods, bufsize, periodsize
        );
        // Not required now that buffer size is set properly
        // swp.set_start_threshold(bufsize - periodsize)?;
        // swp.set_avail_min(periodsize)?;
        // pcm.sw_params(&swp).unwrap();
        info!(
            "Opened audio output {:?} with parameters: {:?}, {:?}",
            dev_name, hwp, swp
        );
    }

    Ok(pcm)
}

impl Open for AlsaSink {
    fn open(device: Option<String>) -> AlsaSink {
        info!("Using alsa sink");

        let name = match device.as_ref().map(AsRef::as_ref) {
            Some("?") => {
                println!("Listing available alsa outputs");
                list_outputs();
                exit(0)
            }
            Some(device) => device,
            None => "default",
        }
        .to_string();

        AlsaSink(None, name)
    }
}

impl Sink for AlsaSink {
    fn start(&mut self) -> io::Result<()> {
        if self.0.is_none() {
            let pcm = open_device(&self.1);
            match pcm {
                Ok(p) => self.0 = Some(p),
                Err(e) => {
                    error!("Alsa error PCM open {}", e);
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "Alsa error: PCM open failed",
                    ));
                }
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> io::Result<()> {
        {
            let pcm = self.0.as_ref().unwrap();
            pcm.drain().unwrap();
        }
        self.0 = None;
        Ok(())
    }

    fn write(&mut self, data: &[i16]) -> io::Result<()> {
        let pcm = self.0.as_mut().unwrap();
        let io = pcm.io_i16().unwrap();

        match io.writei(&data) {
            Ok(_) => (),
            Err(err) => pcm.try_recover(err, false).unwrap(),
            // Err(err) => println!("{:?}",err),
        }

        Ok(())
    }
}
