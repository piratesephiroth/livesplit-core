use crate::{ConsumePreference, Hotkey, KeyCode, Modifiers, Result};
use std::{
    cell::RefCell,
    collections::hash_map::{Entry, HashMap},
    fmt, mem, ptr,
    sync::{
        Arc, Mutex,
        mpsc::{Sender, channel},
    },
    thread,
};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{
            MAPVK_VK_TO_CHAR, MAPVK_VK_TO_VSC_EX, MAPVK_VSC_TO_VK_EX, MapVirtualKeyW,
        },
        WindowsAndMessaging::{
            CallNextHookEx, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
            PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
            WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

const MSG_EXIT: u32 = 0x400;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    WindowsHook,
    ThreadStopped,
    MessageLoop,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WindowsHook => "An error occurred in the Windows API.",
            Self::ThreadStopped => "The background thread stopped unexpectedly.",
            Self::MessageLoop => "An error occurred in the message loop.",
        })
    }
}

type Callback = Box<dyn FnMut() + Send + 'static>;

pub struct Hook {
    thread_id: u32,
    hotkeys: Arc<Mutex<HashMap<Hotkey, Callback>>>,
}

impl Drop for Hook {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, MSG_EXIT, 0, 0);
        }
    }
}

struct State {
    hook: HHOOK,
    events: Sender<Hotkey>,
    modifiers: Modifiers,
    // FIXME: Use variant count when it's stable.
    // https://github.com/rust-lang/rust/issues/73662
    key_state: [u8; 256 / 8],
}

// This static assert ensures we have enough states to represent all key codes.
const _: () = assert!(mem::size_of::<KeyCode>() == 1);

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

const fn parse_scan_code(value: u32) -> Option<KeyCode> {
    // Windows uses PS/2 scan code set 1.
    // https://www.avrfreaks.net/sites/default/files/PS2%20Keyboard.pdf Page 19
    // This is the most up to the date documentation it seems:
    // https://learn.microsoft.com/en-us/windows/win32/inputdev/about-keyboard-input#scan-codes
    use self::KeyCode::*;
    Some(match value {
        0x0001 => Escape,
        0x0002 => Digit1,
        0x0003 => Digit2,
        0x0004 => Digit3,
        0x0005 => Digit4,
        0x0006 => Digit5,
        0x0007 => Digit6,
        0x0008 => Digit7,
        0x0009 => Digit8,
        0x000A => Digit9,
        0x000B => Digit0,
        0x000C => Minus,
        0x000D => Equal,
        0x000E => Backspace,
        0x000F => Tab,
        0x0010 => KeyQ,
        0x0011 => KeyW,
        0x0012 => KeyE,
        0x0013 => KeyR,
        0x0014 => KeyT,
        0x0015 => KeyY,
        0x0016 => KeyU,
        0x0017 => KeyI,
        0x0018 => KeyO,
        0x0019 => KeyP,
        0x001A => BracketLeft,
        0x001B => BracketRight,
        0x001C => Enter,
        0x001D => ControlLeft,
        0x001E => KeyA,
        0x001F => KeyS,
        0x0020 => KeyD,
        0x0021 => KeyF,
        0x0022 => KeyG,
        0x0023 => KeyH,
        0x0024 => KeyJ,
        0x0025 => KeyK,
        0x0026 => KeyL,
        0x0027 => Semicolon,
        0x0028 => Quote,
        0x0029 => Backquote,
        0x002A => ShiftLeft,
        0x002B => Backslash,
        0x002C => KeyZ,
        0x002D => KeyX,
        0x002E => KeyC,
        0x002F => KeyV,
        0x0030 => KeyB,
        0x0031 => KeyN,
        0x0032 => KeyM,
        0x0033 => Comma,
        0x0034 => Period,
        0x0035 => Slash,
        0x0036 => ShiftRight,
        0x0037 => NumpadMultiply,
        0x0038 => AltLeft,
        0x0039 => Space,
        0x003A => CapsLock,
        0x003B => F1,
        0x003C => F2,
        0x003D => F3,
        0x003E => F4,
        0x003F => F5,
        0x0040 => F6,
        0x0041 => F7,
        0x0042 => F8,
        0x0043 => F9,
        0x0044 => F10,
        0x0045 => Pause,
        0x0046 => ScrollLock,
        0x0047 => Numpad7,
        0x0048 => Numpad8,
        0x0049 => Numpad9,
        0x004A => NumpadSubtract,
        0x004B => Numpad4,
        0x004C => Numpad5,
        0x004D => Numpad6,
        0x004E => NumpadAdd,
        0x004F => Numpad1,
        0x0050 => Numpad2,
        0x0051 => Numpad3,
        0x0052 => Numpad0,
        0x0053 => NumpadDecimal,
        0x0054 => PrintScreen, // Not Chrome, Not Safari
        0x0056 => IntlBackslash,
        0x0057 => F11,
        0x0058 => F12,
        0x0059 => NumpadEqual,
        // 0x005C => International6, // Weird カソマ / "Kasoma" key that doesn't even exist.
        0x0064 => F13,
        0x0065 => F14,
        0x0066 => F15,
        0x0067 => F16,
        0x0068 => F17,
        0x0069 => F18,
        0x006A => F19,
        0x006B => F20,
        0x006C => F21,
        0x006D => F22,
        0x006E => F23,
        0x0070 => KanaMode,
        0x0071 => Lang2,
        0x0072 => Lang1,
        0x0073 => IntlRo,
        0x0076 => F24,   // This overlaps with Lang5
        0x0077 => Lang4, // Not Firefox
        0x0078 => Lang3, // Not Firefox
        0x0079 => Convert,
        0x007B => NonConvert,
        0x007D => IntlYen,
        0x007E => NumpadComma,
        // 0x00FC => PostFail, // Not keys
        // 0x00FF => ErrorRollOver, // Not keys
        0xE008 => Undo,  // Not Firefox
        0xE00A => Paste, // Not Firefox
        0xE010 => MediaTrackPrevious,
        0xE017 => Cut,
        0xE018 => Copy,
        0xE019 => MediaTrackNext,
        0xE01C => NumpadEnter,
        0xE01D => ControlRight,
        0xE01E => LaunchMail,
        0xE020 => AudioVolumeMute,
        0xE021 => LaunchApp2,
        0xE022 => MediaPlayPause,
        0xE024 => MediaStop,
        0xE02C => Eject, // Not Firefox
        0xE02E => AudioVolumeDown,
        0xE030 => AudioVolumeUp,
        0xE032 => BrowserHome,
        0xE035 => NumpadDivide,
        0xE036 => ShiftRight, // Somehow reported as extended by the low level hook?!
        0xE037 => PrintScreen,
        0xE038 => AltRight,
        0xE03B => Help, // Not Firefox
        0xE045 => NumLock,
        0xE046 => Pause, // Not Safari
        0xE047 => Home,
        0xE048 => ArrowUp,
        0xE049 => PageUp,
        0xE04B => ArrowLeft,
        0xE04D => ArrowRight,
        0xE04F => End,
        0xE050 => ArrowDown,
        0xE051 => PageDown,
        0xE052 => Insert,
        0xE053 => Delete,
        0xE05B => MetaLeft,
        0xE05C => MetaRight,
        0xE05D => ContextMenu,
        0xE05E => Power,
        0xE05F => Sleep,  // Not Firefox
        0xE063 => WakeUp, // Not Firefox
        0xE065 => BrowserSearch,
        0xE066 => BrowserFavorites,
        0xE067 => BrowserRefresh,
        0xE068 => BrowserStop,
        0xE069 => BrowserForward,
        0xE06A => BrowserBack,
        0xE06B => LaunchApp1,
        0xE06C => LaunchMail,
        0xE06D => MediaSelect,
        0xE0F1 => Lang2, // Not Chrome, Not Safari
        0xE0F2 => Lang1, // Not Chrome, Not Safari
        // 0xE11D => Pause, // Not Chrome, Not Safari, Not Firefox
        _ => return None,
    })
}

unsafe extern "system" fn callback_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let hook = STATE.with_borrow_mut(|state| {
        let state = state.as_mut().expect("State should be initialized by now");

        if code >= 0 {
            // SAFETY: We checked whether it's valid. The caller guarantees that
            // lparam is a valid pointer to a KBDLLHOOKSTRUCT.
            let hook_struct = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            let event = wparam as u32;
            if event == WM_KEYDOWN || event == WM_SYSKEYDOWN {
                // Windows in addition to the scan code has a notion of a
                // virtual key code. This however is already dependent on the
                // keyboard layout. So we should prefer the scan code over the
                // virtual key code. It's hard to come by what these scan codes
                // actually mean, but there's a document released by Microsoft
                // that contains most (not all sadly) mappings from USB HID to
                // the scan code (which matches the PS/2 scan code set 1 make
                // column).
                // http://download.microsoft.com/download/1/6/1/161ba512-40e2-4cc9-843a-923143f3456c/translate.pdf
                // Scan codes can come in an extended form `e0/e1 xx`, so you
                // need to check for the extended field in the flags, as the
                // scan code provided by itself is not extended. Also not every
                // key press somehow even has a scan code. It seems like these
                // might be caused by a special keyboard driver that directly
                // emits the virtual key codes for those keys rather than any
                // physical scan codes ever coming in. Windows has a way to
                // translate those back into scan codes though, so this is what
                // we do in that case.
                let scan_code = if hook_struct.scanCode != 0 {
                    hook_struct.scanCode + ((hook_struct.flags & LLKHF_EXTENDED) * 0xE000)
                } else {
                    // SAFETY: Always safe to call.
                    unsafe { MapVirtualKeyW(hook_struct.vkCode, MAPVK_VK_TO_VSC_EX) }
                };

                if let Some(key_code) = parse_scan_code(scan_code) {
                    let (idx, bit) = key_idx(key_code);
                    if state.key_state[idx as usize] & bit == 0 {
                        state.key_state[idx as usize] |= bit;

                        state
                            .events
                            .send(Hotkey {
                                key_code,
                                modifiers: state.modifiers,
                            })
                            .expect("Callback Thread disconnected");

                        match key_code {
                            KeyCode::AltLeft | KeyCode::AltRight => {
                                state.modifiers.insert(Modifiers::ALT);
                            }
                            KeyCode::ControlLeft | KeyCode::ControlRight => {
                                state.modifiers.insert(Modifiers::CONTROL);
                            }
                            KeyCode::MetaLeft | KeyCode::MetaRight => {
                                state.modifiers.insert(Modifiers::META);
                            }
                            KeyCode::ShiftLeft | KeyCode::ShiftRight => {
                                state.modifiers.insert(Modifiers::SHIFT);
                            }
                            _ => {}
                        }
                    }
                }
            } else if event == WM_KEYUP || event == WM_SYSKEYUP {
                // Windows in addition to the scan code has a notion of a
                // virtual key code. This however is already dependent on the
                // keyboard layout. So we should prefer the scan code over the
                // virtual key code. It's hard to come by what these scan codes
                // actually mean, but there's a document released by Microsoft
                // that contains most (not all sadly) mappings from USB HID to
                // the scan code (which matches the PS/2 scan code set 1 make
                // column).
                // http://download.microsoft.com/download/1/6/1/161ba512-40e2-4cc9-843a-923143f3456c/translate.pdf
                // Scan codes can come in an extended form `e0/e1 xx`, so you
                // need to check for the extended field in the flags, as the
                // scan code provided by itself is not extended. Also not every
                // key press somehow even has a scan code. It seems like these
                // might be caused by a special keyboard driver that directly
                // emits the virtual key codes for those keys rather than any
                // physical scan codes ever coming in. Windows has a way to
                // translate those back into scan codes though, so this is what
                // we do in that case.
                let scan_code = if hook_struct.scanCode != 0 {
                    hook_struct.scanCode + ((hook_struct.flags & LLKHF_EXTENDED) * 0xE000)
                } else {
                    // SAFETY: Always safe to call.
                    unsafe { MapVirtualKeyW(hook_struct.vkCode, MAPVK_VK_TO_VSC_EX) }
                };

                if let Some(key_code) = parse_scan_code(scan_code) {
                    let (idx, bit) = key_idx(key_code);
                    state.key_state[idx as usize] &= !bit;

                    match key_code {
                        KeyCode::AltLeft | KeyCode::AltRight => {
                            state.modifiers.remove(Modifiers::ALT);
                        }
                        KeyCode::ControlLeft | KeyCode::ControlRight => {
                            state.modifiers.remove(Modifiers::CONTROL);
                        }
                        KeyCode::MetaLeft | KeyCode::MetaRight => {
                            state.modifiers.remove(Modifiers::META);
                        }
                        KeyCode::ShiftLeft | KeyCode::ShiftRight => {
                            state.modifiers.remove(Modifiers::SHIFT);
                        }
                        _ => {}
                    }
                }
            }
        }

        state.hook
    });

    // SAFETY: We are forwarding everything to the next hook as per
    // documentation.
    unsafe { CallNextHookEx(hook, code, wparam, lparam) }
}

#[inline]
fn key_idx(key_code: KeyCode) -> (u8, u8) {
    let value = key_code as u8;
    (value / 8, 1 << (value % 8))
}

impl Hook {
    pub fn new(consume: ConsumePreference) -> Result<Self> {
        if matches!(consume, ConsumePreference::MustConsume) {
            return Err(crate::Error::UnmatchedPreference);
        }

        let hotkeys = Arc::new(Mutex::new(HashMap::<
            Hotkey,
            Box<dyn FnMut() + Send + 'static>,
        >::new()));

        let (initialized_tx, initialized_rx) = channel();
        let (events_tx, events_rx) = channel();

        thread::spawn(move || {
            let mut hook = ptr::null_mut();

            STATE.with(|state| {
                hook = unsafe {
                    SetWindowsHookExW(
                        WH_KEYBOARD_LL,
                        Some(callback_proc),
                        GetModuleHandleW(ptr::null()),
                        0,
                    )
                };

                if !hook.is_null() {
                    initialized_tx
                        .send(Ok(unsafe { GetCurrentThreadId() }))
                        .map_err(|_| Error::ThreadStopped)?;
                } else {
                    initialized_tx
                        .send(Err(crate::Error::Platform(Error::WindowsHook)))
                        .map_err(|_| Error::ThreadStopped)?;
                }

                *state.borrow_mut() = Some(State {
                    hook,
                    events: events_tx,
                    modifiers: Modifiers::empty(),
                    key_state: Default::default(),
                });

                Ok(())
            })?;

            loop {
                let mut msg = mem::MaybeUninit::uninit();
                let ret = unsafe { GetMessageW(msg.as_mut_ptr(), ptr::null_mut(), 0, 0) };
                if ret < 0 {
                    return Err(Error::MessageLoop);
                }
                if unsafe { msg.assume_init().message } == MSG_EXIT {
                    break;
                }
            }

            unsafe {
                UnhookWindowsHookEx(hook);
            }

            Ok(())
        });

        let hotkey_map = hotkeys.clone();

        thread::spawn(move || {
            while let Ok(key) = events_rx.recv() {
                if let Some(callback) = hotkey_map.lock().unwrap().get_mut(&key) {
                    callback();
                }
            }
        });

        let thread_id = initialized_rx
            .recv()
            .map_err(|_| crate::Error::Platform(Error::ThreadStopped))??;

        Ok(Hook { thread_id, hotkeys })
    }

    pub fn register<F>(&self, hotkey: Hotkey, callback: F) -> Result<()>
    where
        F: FnMut() + Send + 'static,
    {
        if let Entry::Vacant(vacant) = self.hotkeys.lock().unwrap().entry(hotkey) {
            vacant.insert(Box::new(callback));
            Ok(())
        } else {
            Err(crate::Error::AlreadyRegistered)
        }
    }

    pub fn unregister(&self, hotkey: Hotkey) -> Result<()> {
        if self.hotkeys.lock().unwrap().remove(&hotkey).is_some() {
            Ok(())
        } else {
            Err(crate::Error::NotRegistered)
        }
    }

    pub fn try_resolve(&self, key_code: KeyCode) -> Option<String> {
        use self::KeyCode::*;
        let scan_code = match key_code {
            Backquote => 0x0029,
            Backslash => 0x002B,
            BracketLeft => 0x001A,
            BracketRight => 0x001B,
            Comma => 0x0033,
            Digit1 => 0x0002,
            Digit2 => 0x0003,
            Digit3 => 0x0004,
            Digit4 => 0x0005,
            Digit5 => 0x0006,
            Digit6 => 0x0007,
            Digit7 => 0x0008,
            Digit8 => 0x0009,
            Digit9 => 0x000A,
            Digit0 => 0x000B,
            Equal => 0x000D,
            IntlBackslash => 0x0056,
            IntlRo => 0x0073,
            IntlYen => 0x007D,
            KeyA => 0x001E,
            KeyB => 0x0030,
            KeyC => 0x002E,
            KeyD => 0x0020,
            KeyE => 0x0012,
            KeyF => 0x0021,
            KeyG => 0x0022,
            KeyH => 0x0023,
            KeyI => 0x0017,
            KeyJ => 0x0024,
            KeyK => 0x0025,
            KeyL => 0x0026,
            KeyM => 0x0032,
            KeyN => 0x0031,
            KeyO => 0x0018,
            KeyP => 0x0019,
            KeyQ => 0x0010,
            KeyR => 0x0013,
            KeyS => 0x001F,
            KeyT => 0x0014,
            KeyU => 0x0016,
            KeyV => 0x002F,
            KeyW => 0x0011,
            KeyX => 0x002D,
            KeyY => 0x0015,
            KeyZ => 0x002C,
            Minus => 0x000C,
            Period => 0x0034,
            Quote => 0x0028,
            Semicolon => 0x0027,
            Slash => 0x0035,
            _ => return None,
        };

        let virtual_key_code = unsafe { MapVirtualKeyW(scan_code, MAPVK_VSC_TO_VK_EX) };
        if virtual_key_code == 0 {
            return None;
        }

        let mapped_char = unsafe { MapVirtualKeyW(virtual_key_code, MAPVK_VK_TO_CHAR) };
        if mapped_char == 0 {
            return None;
        }

        // Dead keys (diacritics) are indicated by setting the top bit of the return
        // value.
        const TOP_BIT_MASK: u32 = u32::MAX >> 1;
        let char = mapped_char & TOP_BIT_MASK;

        Some(char::from_u32(char)?.to_string())
    }
}
