use clap::Parser;
use rusb::{Context, DeviceHandle, UsbContext};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// Device Constants from your C Driver
const VENDOR_ID: u16 = 0x2ce3;
const PRODUCT_ID: u16 = 0x3828;
const INTERFACE_NUM: u8 = 1;
const EP_IN: u8 = 0x81;
const EP_OUT: u8 = 0x01;

#[derive(Parser, Debug)]
#[command(author, version, about = "All-in-One UseePlus USB Driver & Server")]
struct Args {
    #[arg(short, long, default_value_t = 10.0)]
    fps: f64,
}

/// Instantly fixes ALL malformed 130-byte hardware DQT headers in a frame
fn fix_dqt(jpeg: &[u8]) -> Vec<u8> {
    let mut fixed = Vec::with_capacity(jpeg.len() + 4);
    let mut current_slice = jpeg;

    while let Some(pos) = current_slice.windows(4).position(|w| w == b"\xff\xdb\x00\x82") {
        if pos + 4 + 128 <= current_slice.len() {
            fixed.extend_from_slice(&current_slice[..pos]);
            fixed.extend_from_slice(b"\xff\xdb\x00\x84\x00"); 
            fixed.extend_from_slice(&current_slice[pos + 4..pos + 68]);
            fixed.push(0x01); 
            fixed.extend_from_slice(&current_slice[pos + 68..pos + 132]);
            current_slice = &current_slice[pos + 132..];
        } else {
            fixed.extend_from_slice(&current_slice[..=pos]);
            current_slice = &current_slice[pos + 1..];
        }
    }
    fixed.extend_from_slice(current_slice);
    fixed
}

fn open_usb_device<T: UsbContext>(context: &mut T) -> Option<DeviceHandle<T>> {
    let devices = context.devices().unwrap();
    for device in devices.iter() {
        let desc = device.device_descriptor().unwrap();
        if desc.vendor_id() == VENDOR_ID && desc.product_id() == PRODUCT_ID {
            match device.open() {
                Ok(handle) => return Some(handle),
                Err(e) => eprintln!("Found device, but failed to open: {}", e),
            }
        }
    }
    None
}

fn usb_reader_thread(shared_data: Arc<(Mutex<(u64, Vec<u8>)>, Condvar)>) {
    let mut context = Context::new().unwrap();
    
    let handle = loop {
        if let Some(h) = open_usb_device(&mut context) {
            println!("USB Camera Found and Opened!");
            break h;
        }
        thread::sleep(Duration::from_secs(1));
    };

    let _ = handle.set_auto_detach_kernel_driver(true);
    
    if let Err(e) = handle.claim_interface(INTERFACE_NUM) {
        eprintln!("Failed to claim USB interface: {}", e);
        return;
    }
    
    if let Err(e) = handle.set_alternate_setting(INTERFACE_NUM, 1) {
        eprintln!("Failed to set alternate setting: {}", e);
        return;
    }

    let _ = handle.clear_halt(EP_OUT);
    let _ = handle.clear_halt(EP_IN);

    thread::sleep(Duration::from_millis(100));

    let connect_cmd = [0xbb, 0xaa, 0x05, 0x00, 0x00];
    match handle.write_bulk(EP_OUT, &connect_cmd, Duration::from_secs(2)) {
        Ok(bytes) => println!("Magic init command sent ({} bytes).", bytes),
        Err(e) => eprintln!("Failed to send init command: {}", e),
    }

    let mut buffer = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut frame_count = 0u64;

    println!("Starting bulk data capture...");

    loop {
        match handle.read_bulk(EP_IN, &mut chunk, Duration::from_millis(250)) {
            Ok(size) => {
                let mut payload = &chunk[..size];

                // STRIP THE PROPRIETARY CAMERA HEADER
                // If we don't do this, 12 bytes of garbage get injected into the middle of the JPEG!
                if payload.len() >= 12 && payload[0] == 0xaa && payload[1] == 0xbb && payload[2] == 0x07 {
                    payload = &payload[12..];
                }

                buffer.extend_from_slice(payload);

                // Extract and fix JPEG frames
                loop {
                    let soi = buffer.windows(2).position(|w| w == b"\xff\xd8");
                    if soi.is_none() {
                        if buffer.len() > 2 {
                            let len = buffer.len();
                            buffer.drain(0..len - 2);
                        }
                        break;
                    }
                    let soi = soi.unwrap();

                    let eoi = buffer[soi..].windows(2).position(|w| w == b"\xff\xd9");
                    if eoi.is_none() {
                        if buffer.len() > 1024 * 512 {
                            let len = buffer.len();
                            buffer.drain(0..len - 2);
                        }
                        break;
                    }
                    let eoi = soi + eoi.unwrap();

                    let raw_jpg = &buffer[soi..eoi + 2];
                    let fixed_jpg = fix_dqt(raw_jpg);
                    
                    frame_count += 1;
                    let (lock, cvar) = &*shared_data;
                    {
                        let mut state = lock.lock().unwrap();
                        *state = (frame_count, fixed_jpg);
                    }
                    cvar.notify_all();

                    buffer.drain(0..eoi + 2);
                }
            },
            Err(rusb::Error::Timeout) => {
                continue;
            },
            Err(e) => {
                eprintln!("USB Read Error: {:?}", e);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn handle_client(mut stream: TcpStream, shared_data: Arc<(Mutex<(u64, Vec<u8>)>, Condvar)>, target_fps: f64) {
    let _ = stream.set_nodelay(true);
    let header = b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\n\r\n";
    if stream.write_all(header).is_err() { return; }

    let (lock, cvar) = &*shared_data;
    let mut last_frame_id = 0u64;
    let frame_time = Duration::from_secs_f64(1.0 / target_fps);

    loop {
        let loop_start = Instant::now();

        let frame_data = {
            let mut state = lock.lock().unwrap();
            while state.0 == last_frame_id {
                state = cvar.wait(state).unwrap();
            }
            last_frame_id = state.0;
            state.1.clone()
        };

        let header = format!("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n", frame_data.len());
        if stream.write_all(header.as_bytes()).is_err() || 
           stream.write_all(&frame_data).is_err() || 
           stream.write_all(b"\r\n").is_err() { break; }

        let elapsed = loop_start.elapsed();
        if elapsed < frame_time {
            thread::sleep(frame_time - elapsed);
        }
    }
}

fn main() {
    let args = Args::parse();
    let shared_data = Arc::new((Mutex::new((0u64, Vec::new())), Condvar::new()));
    
    let data_clone = Arc::clone(&shared_data);
    thread::spawn(move || usb_reader_thread(data_clone));

    println!("All-in-One USB MJPEG Server running on http://127.0.0.1:8080 at {} FPS", args.fps);

    let listener = TcpListener::bind("127.0.0.1:8080").unwrap();
    for stream in listener.incoming() {
        if let Ok(stream) = stream {
            let data_clone = Arc::clone(&shared_data);
            let fps = args.fps;
            thread::spawn(move || handle_client(stream, data_clone, fps));
        }
    }
}
