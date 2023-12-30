//! This example shows how to use USB (Universal Serial Bus) in the RP2040 chip.
//!
//! This creates a USB serial port that echos.

#![no_std]
#![no_main]

use core::cell::RefCell;
use defmt::{info, panic};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{AnyPin, Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::{Driver, Instance, InterruptHandler};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::Instant;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::driver::EndpointError;
use embassy_usb::{Builder, Config};
use {defmt_rtt as _, panic_probe as _};

use defmt::*;
use embassy_rp::spi::Spi;
use embassy_rp::spi;

mod max7219;

use crate::max7219::Max7219;

static FRAMEBUF: Mutex<ThreadModeRawMutex, RefCell<[u8; 128]>> =
    Mutex::new(RefCell::new([0u8; 128]));
static VSYNC: Signal<ThreadModeRawMutex, u32> = Signal::new();

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

async fn update_fb(
    shared_fb: &'static Mutex<ThreadModeRawMutex, RefCell<[u8; 128]>>,
    value: &[u8],
) {
    let fb_lock = shared_fb.lock().await;

    let mut fb = fb_lock.borrow_mut();
    for i in 0..128 {
        fb[i] = value[i];
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Hello there!");

    let p = embassy_rp::init(Default::default());

    let mosi = p.PIN_11;
    let clk = p.PIN_10;

    // create SPI
    let mut config = spi::Config::default();
    config.frequency = 2_000_000;
    let spi = Spi::new_txonly(p.SPI1, clk, mosi, p.DMA_CH0, config);

    // Configure CS
    let cs = Output::new(Into::<AnyPin>::into(p.PIN_13), Level::Low);

    let mut max7219 = Max7219::new(spi, cs, 4, 4);

    max7219.send_cmd_to_all(0x09, 0x00).await.unwrap();
    max7219.send_cmd_to_all(0x0A, 0x01).await.unwrap();
    max7219.send_cmd_to_all(0x0B, 0x07).await.unwrap();
    max7219.send_cmd_to_all(0x0C, 0x01).await.unwrap();
    max7219.send_cmd_to_all(0x0F, 0x00).await.unwrap();

    debug!("Spawning MAX7219 thread");
    spawner
        .spawn(max7219::max7219_task(max7219, &FRAMEBUF, &VSYNC))
        .unwrap();

    let driver = Driver::new(p.USB, Irqs);

    // Create embassy-usb Config
    let mut config = Config::new(0xc0de, 0xcafe);
    config.manufacturer = Some("SiliconForest");
    config.product = Some("HUB75 USB");
    config.serial_number = Some("12345678");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    // Required for windows compatibility.
    // https://developer.nordicsemi.com/nRF_Connect_SDK/doc/1.9.1/kconfig/CONFIG_CDC_ACM_IAD.html#help
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;

    // Create embassy-usb DeviceBuilder using the driver and config.
    // It needs some buffers for building the descriptors.
    let mut device_descriptor = [0; 256];
    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut control_buf = [0; 64];

    let mut state = State::new();

    let mut builder = Builder::new(
        driver,
        config,
        &mut device_descriptor,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut [], // no msos descriptors
        &mut control_buf,
    );

    // Create classes on the builder.
    let mut class = CdcAcmClass::new(&mut builder, &mut state, 64);

    // Build the builder.
    let mut usb = builder.build();

    // Run the USB device.
    let usb_fut = usb.run();

    // Do stuff with the class!
    let echo_fut = async {
        loop {
            class.wait_connection().await;
            info!("Connected");
            let _ = echo(&mut class).await;
            info!("Disconnected");
        }
    };

    // Run everything concurrently.
    // If we had made everything `'static` above instead, we could do this using separate tasks instead.
    join(usb_fut, echo_fut).await;
}

struct Disconnected {}

impl From<EndpointError> for Disconnected {
    fn from(val: EndpointError) -> Self {
        match val {
            EndpointError::BufferOverflow => panic!("Buffer overflow"),
            EndpointError::Disabled => Disconnected {},
        }
    }
}

async fn echo<'d, T: Instance + 'd>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
) -> Result<(), Disconnected> {
    let mut buf = [0; 64];

    let mut fb_buf = [0u8; 128];
    let mut fb_cursor = 0;

    let mut last_ts = Instant::now();
    loop {
        let n = class.read_packet(&mut buf).await?;
        let data = &buf[..n];

        let now = Instant::now();
        debug!("Millies: {}", now.duration_since(last_ts).as_millis());
        if now.duration_since(last_ts).as_millis() > 50 {
            fb_cursor = 0;
        }

        last_ts = now;

        for dat in data {
            fb_buf[fb_cursor] = *dat;
            //trace!("cursor: {:x}", fb_cursor);
            fb_cursor += 1;

            if fb_cursor >= 128 {
                update_fb(&FRAMEBUF, &fb_buf).await;
                VSYNC.signal(0);
                fb_cursor = 0;
            }
        }
    }
}
