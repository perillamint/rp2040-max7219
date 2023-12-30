use core::cell::RefCell;
use embassy_rp::gpio::AnyPin;
use embassy_rp::peripherals::SPI1;
use embassy_rp::spi::{Async, Spi};
use embassy_rp::{gpio, spi};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use gpio::Output;
use {defmt_rtt as _, panic_probe as _};

pub struct Max7219<'a> {
    spi: Spi<'a, SPI1, Async>,
    cs: Output<'static, AnyPin>,
    panel_width: usize,
    panel_height: usize,
}

impl<'a> Max7219<'a> {
    pub fn new(
        spi: Spi<'a, SPI1, Async>,
        cs: Output<'static, AnyPin>,
        panel_width: usize,
        panel_height: usize,
    ) -> Self {
        Self {
            spi,
            cs,
            panel_width,
            panel_height,
        }
    }

    pub async fn send_cmd_to_all(&mut self, cmd: u8, data: u8) -> Result<(), spi::Error> {
        let cmd = [cmd, data];

        self.cs.set_low();
        for _ in 0..(self.panel_width * self.panel_height) {
            self.spi.write(&cmd).await?;
        }
        self.cs.set_high();

        Ok(())
    }

    pub async fn send_raw_cmd(&mut self, data: &[u8]) -> Result<(), spi::Error> {
        self.cs.set_low();
        let ret = self.spi.write(data).await;
        self.cs.set_high();

        ret
    }

    pub async fn send_framebuffer(&mut self, fb: &[u8]) -> Result<(), spi::Error> {
        let mut cmdbuf: [u8; 2] = [0x00; 2];
        for row in 0..8 {
            self.cs.set_low();
            for i in 0..(self.panel_width * self.panel_height) {
                cmdbuf[0] = row + 1 as u8;
                cmdbuf[1] = fb[i % (self.panel_width)
                    + i / (self.panel_width) * 32
                    + (row as usize) * (self.panel_height)]
                    .reverse_bits();

                self.spi.write(&cmdbuf).await?;
            }
            self.cs.set_high();
        }

        Ok(())
    }
}

pub async fn send_fb(
    max7219: &mut Max7219<'static>,
    shared_fb: &'static Mutex<ThreadModeRawMutex, RefCell<[u8; 128]>>,
) {
    let fb_lock = shared_fb.lock().await;
    let fb = fb_lock.borrow();
    max7219.send_framebuffer(fb.as_slice()).await.unwrap();
}

#[embassy_executor::task]
pub async fn max7219_task(
    mut max7219: Max7219<'static>,
    shared_fb: &'static Mutex<ThreadModeRawMutex, RefCell<[u8; 128]>>,
    vsync: &'static Signal<ThreadModeRawMutex, u32>,
) {
    loop {
        send_fb(&mut max7219, shared_fb).await;
        vsync.wait().await;
    }
}
