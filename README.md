# RusteePLUS: Zero-Latency User-Space Driver & MJPEG Server

A lightning-fast, pure Rust user-space driver and streaming server for proprietary "UseePlus" / "Geek szitman" budget USB cameras, microscopes, and endoscopes (USB ID 2ce3:3828).

## The Problem
Many inexpensive USB wire cameras and microscopes on the market claim to be standard webcams, but they completely violate the USB Video Class (UVC) specification:
1. They require a proprietary magic byte sequence (0xBB 0xAA 0x05 0x00 0x00) to wake up.
2. They inject 12-byte proprietary headers (AA BB 07...) directly into the middle of the JPEG image payloads, corrupting the frames.
3. The hardware's Image Signal Processor (ISP) generates mathematically impossible JPEG Define Quantization Table (DQT) headers (length 130 instead of 132), causing standard decoders like FFmpeg to drop the frames entirely.

## The Solution
Instead of fighting with complex, crash-prone C kernel modules or using heavy software re-encoding (like OpenCV), RusteePlus handles everything in user-space with virtually zero overhead.

It uses libusb to talk directly to the hardware's bulk endpoints. It strips the garbage headers, performs bit-level surgery to inject the missing bytes into the malformed DQT headers, and instantly serves the pristine JPEG frames over a paced TCP MJPEG stream. 

## Features
- User-Space Safe: No kernel panics, no DKMS modules to rebuild when you update your OS.
- Zero-Latency DQT Surgery: Fixes broken hardware JPEGs by directly mutating the byte slices in nanoseconds.
- Paced MJPEG Server: Smooths out the hardware's wildly fluctuating framerate into a steady, reliable stream.
- Ultra-Lightweight: No heavy dependencies like OpenCV or FFmpeg.

## Prerequisites

You will need the standard Rust toolchain installed (via rustup), as well as libusb and pkg-config for your specific distribution:

Arch Linux:
```bash
sudo pacman -S libusb pkgconf gcc
```

Debian / Ubuntu:
```bash
sudo apt install libusb-1.0-0-dev pkg-config build-essential
```

Fedora:
```bash
sudo dnf install libusb1-devel pkgconf-pkg-config gcc
```

## Installation & Setup

1. Clone the repository:
git clone https://github.com/sirajperson/rusteeplus-linux-driver.git
cd rusteeplus-linux-driver

2. Remove any conflicting kernel modules:
If you were previously using a custom C driver, unload it so it doesn't fight for the device:
sudo rmmod supercamera_simple

3. Set up udev rules (Linux):
By default, Linux requires root to access raw USB devices. Add a udev rule to allow your standard user account to run the driver:
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="2ce3", ATTR{idProduct}=="3828", MODE="0666"' | sudo tee /etc/udev/rules.d/99-useeplus.rules
sudo udevadm control --reload-rules
sudo udevadm trigger
(Note: Unplug the camera and plug it back in after running this).

## Usage

Start the server. You can specify the target framerate (default is 10) to match the exposure speed of your camera environment.

cargo run --release -- --fps 10

### Viewing the Stream
Once the server is running and the camera is initialized, you can view the stream at http://127.0.0.1:8080. 

For the absolute lowest latency and smoothest playback, connect using mpv with the following flags:

mpv http://127.0.0.1:8080/ --profile=low-latency --untimed --no-correct-pts

## Technical Architecture
* USB Reader Thread: Connects to Interface 1, Alt-Setting 1. Blasts the init sequence to EP_OUT (0x01) and captures bulk packets from EP_IN (0x81).
* Byte Slicer: Scans the incoming ring buffer for the AA BB 07 headers and removes them, then slices the stream at FF D8 (SOI) and FF D9 (EOI) markers.
* Header Surgery: Locates the impossible FF DB 00 82 DQT header and injects 0x00 and 0x01 Table IDs to restore it to the spec-compliant 132 byte length.
* TCP Metronome Thread: Uses a Condition Variable (Condvar) to instantly wake up and capture the newest clean frame, serving it exactly at the requested framerate to prevent player starvation.

## License
MIT License
(C) Siraj Musawwir 2026
