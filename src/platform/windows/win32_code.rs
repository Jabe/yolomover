//! Normalize Win32 / HRESULT codes from `windows::core::Error`.

/// Map `HRESULT` (`0x80070057`) or raw Win32 (`87`) to the low Win32 code.
pub(crate) fn win32_code(raw: i32) -> u32 {
    let code = raw as u32;
    if (code & 0xFFFF_0000) == 0x8007_0000 {
        code & 0xFFFF
    } else {
        code
    }
}
