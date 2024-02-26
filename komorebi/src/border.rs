use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

use color_eyre::Result;
use komorebi_core::Rect;
use parking_lot::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::DispatchMessageW;
use windows::Win32::UI::WindowsAndMessaging::GetMessageW;
use windows::Win32::UI::WindowsAndMessaging::CS_HREDRAW;
use windows::Win32::UI::WindowsAndMessaging::CS_VREDRAW;
use windows::Win32::UI::WindowsAndMessaging::HWND_NOTOPMOST;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::Win32::UI::WindowsAndMessaging::WNDCLASSW;

use crate::set_window_position::SetWindowPosition;
use crate::window::Window;
use crate::windows_callbacks;
use crate::WindowsApi;
use crate::BORDER_OFFSET;
use crate::BORDER_WIDTH;
use crate::TRANSPARENCY_COLOUR;

#[derive(Debug)]
pub struct BorderWindow {
    hwnd: HWND,
    enabled: AtomicBool,
    thread: JoinHandle<Result<()>>,
    rect: Mutex<Rect>,
}

impl BorderWindow {
    pub fn new(name: &str) -> Result<Self> {
        let name: Vec<u16> = format!("{name}\0").encode_utf16().collect();
        let instance = WindowsApi::module_handle_w()?;
        let class_name = PCWSTR(name.as_ptr());
        let brush = WindowsApi::create_solid_brush(TRANSPARENCY_COLOUR);
        let window_class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: class_name,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(windows_callbacks::border_window),
            hbrBackground: brush,
            ..Default::default()
        };

        let _atom = WindowsApi::register_class_w(&window_class)?;

        let (tx, rx) = crossbeam_channel::bounded(1);

        let thread = std::thread::spawn(move || -> Result<()> {
            let hwnd = WindowsApi::create_border_window(PCWSTR(name.as_ptr()), instance)?;
            tx.send(hwnd).unwrap();
            std::mem::forget(tx);
            let mut message = MSG::default();
            unsafe {
                while GetMessageW(&mut message, HWND(hwnd), 0, 0).into() {
                    DispatchMessageW(&message);
                }
            }
            Ok(())
        });

        let hwnd = HWND(rx.recv()?);

        Ok(Self {
            hwnd,
            thread,
            enabled: true.into(),
            rect: Default::default(),
        })
    }

    pub fn thread(&self) -> &JoinHandle<Result<()>> {
        &self.thread
    }

    pub fn hide(&self) -> Result<()> {
        WindowsApi::set_window_pos(
            self.hwnd,
            &Default::default(),
            Default::default(),
            SetWindowPosition::HIDE_WINDOW.bits(),
        )
    }

    pub fn set_position(&self, window: Window, activate: bool) -> Result<()> {
        if !self.enabled.load(Ordering::SeqCst) {
            return Ok(());
        }

        let mut rect = WindowsApi::window_rect(window.hwnd())?;
        rect.add_padding(-BORDER_OFFSET.load(Ordering::SeqCst));

        let border_width = BORDER_WIDTH.load(Ordering::SeqCst);
        rect.add_margin(border_width);

        *self.rect.lock() = rect;

        let flags = if activate {
            SetWindowPosition::SHOW_WINDOW | SetWindowPosition::NO_ACTIVATE
        } else {
            SetWindowPosition::NO_ACTIVATE
        };

        // TODO(raggi): This leaves the window behind the active window, which
        // can result e.g. single pixel window borders being invisible in the
        // case of opaque window borders (e.g. EPIC Games Launcher). Ideally
        // we'd be able to pass a parent window to place ourselves just in front
        // of, however the SetWindowPos API explicitly ignores that parameter
        // unless the window being positioned is being activated - and we don't
        // want to activate the border window here. We can hopefully find a
        // better workaround in the future.
        // The trade-off chosen prevents the border window from sitting over the
        // top of other pop-up dialogs such as a file picker dialog from
        // Firefox. When adjusting this in the future, it's important to check
        // those dialog cases.
        let position = HWND_NOTOPMOST;
        WindowsApi::set_window_pos(self.hwnd, &rect, position, flags.bits())
    }

    pub fn rect(&self) -> Rect {
        *self.rect.lock()
    }

    pub fn invalidate_rect(&self) -> Result<()> {
        WindowsApi::invalidate_rect(self.hwnd)
    }

    pub fn disable(&self) {
        if self.enabled.swap(false, Ordering::SeqCst) {
            if let Err(e) = self.hide() {
                tracing::error!("Failed to hide border window: {}", e);
            }
        }
    }

    pub fn enable(&self) {
        if self.enabled.swap(true, Ordering::SeqCst) {
            return;
        }

        match WindowsApi::foreground_window() {
            Ok(window) => {
                if let Err(e) = self.set_position(Window { hwnd: window }, false) {
                    tracing::error!("Failed to set position of border window: {}", e);
                }
            }
            Err(e) => {
                tracing::error!("Failed to get foreground window: {}", e);
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
}
