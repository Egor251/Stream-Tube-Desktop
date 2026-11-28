//! D3D11 render target for libmpv. mpv renders the video on the GPU into
//! `render_tex`; we copy it into a staging texture and read it back as plain
//! BGRA pixels for GPUI's image atlas. This avoids mpv's slow software
//! YUV→RGB conversion entirely, and keeps video inside the main GPUI window
//! so overlays and mouse input keep working.
#![cfg(windows)]

use anyhow::{Result, anyhow};
use std::ffi::c_void;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};

pub struct GpuState {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    render_tex: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
}

fn texture_desc(w: u32, h: u32, usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE,
                bind: u32, cpu: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: cpu,
        MiscFlags: 0,
    }
}

impl GpuState {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| anyhow!("D3D11CreateDevice: {e}"))?;
            let device = device.ok_or_else(|| anyhow!("no D3D11 device"))?;
            let context = context.ok_or_else(|| anyhow!("no D3D11 context"))?;

            let mut render_tex: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(
                &texture_desc(width, height, D3D11_USAGE_DEFAULT,
                              (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32, 0),
                None,
                Some(&mut render_tex),
            ).map_err(|e| anyhow!("create render texture: {e}"))?;
            let render_tex = render_tex.ok_or_else(|| anyhow!("no render texture"))?;

            let mut staging: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(
                &texture_desc(width, height, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32),
                None,
                Some(&mut staging),
            ).map_err(|e| anyhow!("create staging texture: {e}"))?;
            let staging = staging.ok_or_else(|| anyhow!("no staging texture"))?;

            Ok(Self { device, context, render_tex, staging, width, height })
        }
    }

    pub fn device_ptr(&self) -> *mut c_void {
        unsafe { std::mem::transmute_copy::<ID3D11Device, *mut c_void>(&self.device) }
    }

    pub fn texture_ptr(&self) -> *mut c_void {
        unsafe { std::mem::transmute_copy::<ID3D11Texture2D, *mut c_void>(&self.render_tex) }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Copies the rendered frame into CPU memory as tightly packed BGRA.
    pub fn readback(&self) -> Result<Vec<u8>> {
        unsafe {
            self.context.CopyResource(&self.staging, &self.render_tex);
            let mut mapped = Default::default();
            self.context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| anyhow!("map staging: {e}"))?;
            let src = mapped.pData as *const u8;
            let stride = mapped.RowPitch as usize;
            let width = self.width as usize * 4;
            let mut out = vec![0u8; width * self.height as usize];
            for y in 0..self.height as usize {
                std::ptr::copy_nonoverlapping(src.add(y * stride), out.as_mut_ptr().add(y * width), width);
            }
            self.context.Unmap(&self.staging, 0);
            // Keep BGRA byte order: GPUI's Windows atlas uploads these bytes
            // as-is (the old software path produced BGR0 the same way).
            for px in out.chunks_exact_mut(4) {
                px[3] = 0xFF;
            }
            Ok(out)
        }
    }
}
