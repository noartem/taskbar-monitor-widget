//! Minimal HTTPS GET over WinHTTP: system TLS, system proxy settings, no extra dependencies.

use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS,
    WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_RETRY_AFTER, WINHTTP_QUERY_STATUS_CODE,
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
};
use windows::core::{HSTRING, PCWSTR};

pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
    /// Seconds from a `Retry-After` header.
    pub retry_after: Option<u64>,
}

struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn check(h: *mut core::ffi::c_void, what: &str) -> Result<Handle, String> {
    if h.is_null() { Err(format!("{what} failed: {}", windows::core::Error::from_thread())) } else { Ok(Handle(h)) }
}

/// Splits `https://host[:port]/path` into (secure, host, port, path).
pub fn split_url(url: &str) -> Result<(bool, String, u16, String), String> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("unsupported URL: {url}"));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| format!("bad port in {url}"))?),
        None => (authority, if secure { 443 } else { 80 }),
    };
    Ok((secure, host.to_string(), port, path.to_string()))
}

pub fn get(url: &str, headers: &[(&str, &str)], user_agent: &str) -> Result<Response, String> {
    let (secure, host, port, path) = split_url(url)?;
    unsafe {
        let session = check(
            WinHttpOpen(&HSTRING::from(user_agent), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
            "WinHttpOpen",
        )?;
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 15_000);
        let connect = check(WinHttpConnect(session.0, &HSTRING::from(host), port, 0), "WinHttpConnect")?;
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = check(
            WinHttpOpenRequest(
                connect.0,
                &HSTRING::from("GET"),
                &HSTRING::from(path),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                flags,
            ),
            "WinHttpOpenRequest",
        )?;
        // Usage requests carry credentials: never forward them to a redirect target.
        let disabled = WINHTTP_DISABLE_REDIRECTS.to_ne_bytes();
        WinHttpSetOption(Some(request.0.cast_const()), WINHTTP_OPTION_DISABLE_FEATURE, Some(&disabled))
            .map_err(|e| format!("redirect policy: {}", e.message()))?;

        let header_text: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        let header_wide: Vec<u16> = header_text.encode_utf16().collect();
        WinHttpSendRequest(request.0, Some(&header_wide), None, 0, 0, 0).map_err(|e| format!("send: {}", e.message()))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| format!("receive: {}", e.message()))?;

        let mut status = 0u32;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut _ as *mut _),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(|e| format!("status: {}", e.message()))?;

        let mut retry = 0u32;
        let mut len = 4u32;
        let retry_after = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_RETRY_AFTER | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut retry as *mut _ as *mut _),
            &mut len,
            std::ptr::null_mut(),
        )
        .ok()
        .map(|_| retry as u64);

        let mut body = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut _, chunk.len() as u32, &mut read)
                .map_err(|e| format!("read: {}", e.message()))?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > 4 * 1024 * 1024 {
                return Err("response too large".into());
            }
        }
        Ok(Response { status, body, retry_after })
    }
}
