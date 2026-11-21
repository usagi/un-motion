use std::{thread, time::{Duration, Instant}};
use tao::{event_loop::{ControlFlow, EventLoop}, platform::windows::WindowExtWindows, window::WindowBuilder};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
fn main() {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new().with_title("UNM isolated input regression").with_visible(false).build(&event_loop).unwrap();
    let hwnd = window.hwnd() as usize;
    thread::spawn(move || loop {
        unsafe {
            PostMessageW(hwnd as _, WM_SYSKEYDOWN, 0x12, 0x20380001);
            PostMessageW(hwnd as _, WM_SYSKEYUP, 0x12, 0xc1380001u32 as isize);
        }
        thread::sleep(Duration::from_micros(500));
    });
    for message in [WM_KILLFOCUS, WM_SETFOCUS] {
        thread::spawn(move || loop {
            let mut result = 0;
            unsafe { SendMessageTimeoutW(hwnd as _, message, 0, 0, SMTO_ABORTIFHUNG, 100, &mut result); }
            thread::sleep(Duration::from_micros(200));
        });
    }
    thread::spawn(move || {
        let start = Instant::now();
        let mut failures = 0;
        let mut replies = 0;
        loop {
            thread::sleep(Duration::from_millis(100));
            let mut result = 0;
            let ok = unsafe { SendMessageTimeoutW(hwnd as _, WM_NULL, 0, 0, SMTO_ABORTIFHUNG, 500, &mut result) } != 0;
            if ok { failures = 0; replies += 1; } else { failures += 1; }
            if failures >= 3 { println!("HUNG replies={replies} elapsed={:?}", start.elapsed()); std::process::exit(1); }
            if start.elapsed() >= Duration::from_secs(20) { println!("RESPONSIVE replies={replies} elapsed={:?}", start.elapsed()); std::process::exit(0); }
        }
    });
    event_loop.run(move |_, _, control| { let _ = &window; *control = ControlFlow::Wait; });
}
