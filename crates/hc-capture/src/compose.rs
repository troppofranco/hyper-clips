//! Tiny D3D11 compositor used on top of the captured frame:
//! - HDR (scRGB FP16) → SDR BT.709 tone-mapping,
//! - mouse cursor and webcam overlays (alpha blended).
//!
//! Shaders are compiled at start-up with the system `d3dcompiler_47.dll`
//! (part of Windows 10/11), so no build-time shader toolchain is needed.

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, PCSTR, s};

use crate::CaptureError;

const HLSL: &str = r#"
cbuffer C : register(b0) { float4 rect; float4 params; };
Texture2D tex : register(t0);
SamplerState smp : register(s0);
struct VO { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
VO vs_main(uint id : SV_VertexID) {
    float2 uv = float2(id & 1, id >> 1);
    VO o;
    o.uv = uv;
    o.pos = float4(lerp(rect.x, rect.z, uv.x), lerp(rect.y, rect.w, uv.y), 0, 1);
    return o;
}
float4 ps_main(VO i) : SV_Target {
    float4 c = tex.Sample(smp, i.uv);
    if (params.x > 0.5) {
        // scRGB (linear BT.709, 1.0 = 80 nits) -> SDR: scale to paper white,
        // soft-knee roll-off on the max channel, then sRGB encode.
        float3 l = max(c.rgb * params.y, 0.0);
        float m = max(max(l.r, l.g), l.b);
        const float k = 0.75;
        if (m > k) {
            float nm = k + (1.0 - k) * (1.0 - exp(-(m - k) / (1.0 - k)));
            l *= nm / m;
        }
        float3 lo = l * 12.92;
        float3 hi = 1.055 * pow(l, 1.0 / 2.4) - 0.055;
        float3 srgb = l <= 0.0031308 ? lo : hi;
        return float4(srgb, 1.0);
    }
    return float4(c.rgb, c.a * params.z);
}
"#;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawMode {
    /// Alpha-blended copy with extra opacity (0..=1).
    Blend { opacity: f32 },
    /// HDR scRGB → SDR. `scale` = 80 / SDR white level in nits.
    ToneMap { scale: f32 },
}

#[repr(C)]
struct Constants {
    rect: [f32; 4],
    params: [f32; 4],
}

pub struct Compositor {
    context: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    raster: ID3D11RasterizerState,
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob, CaptureError> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: source buffer outlives the call; blobs are owned.
    let res = unsafe {
        D3DCompile(
            HLSL.as_ptr().cast(),
            HLSL.len(),
            s!("hc-compose"),
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = res {
        let msg = errors
            .map(|b| {
                // SAFETY: error blob holds an ANSI string of the given size.
                unsafe {
                    String::from_utf8_lossy(std::slice::from_raw_parts(
                        b.GetBufferPointer() as *const u8,
                        b.GetBufferSize(),
                    ))
                    .into_owned()
                }
            })
            .unwrap_or_default();
        return Err(CaptureError::Other(format!(
            "shader compile failed: {e} {msg}"
        )));
    }
    code.ok_or_else(|| CaptureError::Other("empty shader".into()))
}

fn blob_bytes(b: &ID3DBlob) -> &[u8] {
    // SAFETY: blob memory is valid for the blob's lifetime.
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }
}

impl Compositor {
    pub fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Result<Self, CaptureError> {
        let vs_blob = compile(s!("vs_main"), s!("vs_4_0"))?;
        let ps_blob = compile(s!("ps_main"), s!("ps_4_0"))?;
        // SAFETY: D3D11 object creation with fully initialised descriptors.
        unsafe {
            let mut vs = None;
            device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut ps))?;
            let mut cb = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: size_of::<Constants>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut cb),
            )?;
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            let mut rt = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_SRC_ALPHA,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_ZERO,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            rt.BlendEnable = true.into();
            let mut blend_desc = D3D11_BLEND_DESC::default();
            blend_desc.RenderTarget[0] = rt;
            let mut blend = None;
            device.CreateBlendState(&blend_desc, Some(&mut blend))?;
            let mut raster = None;
            device.CreateRasterizerState(
                &D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    CullMode: D3D11_CULL_NONE,
                    DepthClipEnable: true.into(),
                    ..Default::default()
                },
                Some(&mut raster),
            )?;
            let missing = || CaptureError::Other("D3D11 object creation".into());
            Ok(Self {
                context: context.clone(),
                vs: vs.ok_or_else(missing)?,
                ps: ps.ok_or_else(missing)?,
                cb: cb.ok_or_else(missing)?,
                sampler: sampler.ok_or_else(missing)?,
                blend: blend.ok_or_else(missing)?,
                raster: raster.ok_or_else(missing)?,
            })
        }
    }

    /// Draw `src` into `target` at pixel rectangle `dst` = [x0, y0, x1, y1].
    pub fn draw(
        &self,
        target: &RenderTarget,
        src: &ID3D11ShaderResourceView,
        dst: [f32; 4],
        mode: DrawMode,
    ) {
        let (w, h) = (target.width as f32, target.height as f32);
        let ndc = [
            dst[0] / w * 2.0 - 1.0,
            1.0 - dst[1] / h * 2.0,
            dst[2] / w * 2.0 - 1.0,
            1.0 - dst[3] / h * 2.0,
        ];
        let params = match mode {
            DrawMode::Blend { opacity } => [0.0, 1.0, opacity, 0.0],
            DrawMode::ToneMap { scale } => [1.0, scale, 1.0, 0.0],
        };
        let c = Constants { rect: ndc, params };
        let ctx = &self.context;
        // SAFETY: all bound objects belong to the same device and outlive the draw.
        unsafe {
            ctx.UpdateSubresource(&self.cb, 0, None, (&c as *const Constants).cast(), 0, 0);
            ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: w,
                Height: h,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.RSSetState(&self.raster);
            ctx.OMSetBlendState(&self.blend, None, 0xFFFF_FFFF);
            ctx.IASetInputLayout(None);
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.PSSetShaderResources(0, Some(&[Some(src.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.Draw(4, 0);
            // Unbind so the source can be used as a copy target next frame.
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
    }
}

/// A BGRA texture we can render into and feed to the video processor.
pub struct RenderTarget {
    pub texture: ID3D11Texture2D,
    pub rtv: ID3D11RenderTargetView,
    pub width: u32,
    pub height: u32,
}

impl RenderTarget {
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self, CaptureError> {
        // SAFETY: plain texture + view creation.
        unsafe {
            let mut tex = None;
            device.CreateTexture2D(
                &texture_desc(
                    width,
                    height,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0,
                ),
                None,
                Some(&mut tex),
            )?;
            let tex: ID3D11Texture2D =
                tex.ok_or_else(|| CaptureError::Other("render target".into()))?;
            let mut rtv = None;
            device.CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
            Ok(Self {
                rtv: rtv.ok_or_else(|| CaptureError::Other("RTV".into()))?,
                texture: tex,
                width,
                height,
            })
        }
    }
}

pub fn texture_desc(
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    bind: i32,
) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    }
}

/// A CPU-updatable BGRA overlay (cursor, webcam).
pub struct Overlay {
    pub texture: ID3D11Texture2D,
    pub srv: ID3D11ShaderResourceView,
    pub width: u32,
    pub height: u32,
}

impl Overlay {
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self, CaptureError> {
        // SAFETY: plain texture + view creation.
        unsafe {
            let mut tex = None;
            device.CreateTexture2D(
                &texture_desc(
                    width,
                    height,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    D3D11_BIND_SHADER_RESOURCE.0,
                ),
                None,
                Some(&mut tex),
            )?;
            let tex: ID3D11Texture2D = tex.ok_or_else(|| CaptureError::Other("overlay".into()))?;
            let srv = srv_for(device, &tex)?;
            Ok(Self {
                texture: tex,
                srv,
                width,
                height,
            })
        }
    }

    /// Upload tightly packed BGRA pixels.
    pub fn upload(&self, context: &ID3D11DeviceContext, bgra: &[u8]) {
        if bgra.len() < (self.width * self.height * 4) as usize {
            return;
        }
        // SAFETY: buffer size checked above.
        unsafe {
            context.UpdateSubresource(
                &self.texture,
                0,
                None,
                bgra.as_ptr().cast(),
                self.width * 4,
                0,
            );
        }
    }
}

pub fn srv_for(
    device: &ID3D11Device,
    tex: &ID3D11Texture2D,
) -> Result<ID3D11ShaderResourceView, CaptureError> {
    let mut srv = None;
    // SAFETY: texture was created with BIND_SHADER_RESOURCE.
    unsafe {
        device.CreateShaderResourceView(&tex.cast::<ID3D11Resource>()?, None, Some(&mut srv))?
    };
    srv.ok_or_else(|| CaptureError::Other("SRV".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::create_device;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT;

    fn read_pixel(
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        tex: &ID3D11Texture2D,
    ) -> [u8; 4] {
        // SAFETY: staging readback.
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut st = None;
            device.CreateTexture2D(&desc, None, Some(&mut st)).unwrap();
            let st = st.unwrap();
            ctx.CopyResource(&st, tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m)).unwrap();
            let p = std::slice::from_raw_parts(m.pData as *const u8, 4);
            let px = [p[0], p[1], p[2], p[3]];
            ctx.Unmap(&st, 0);
            px
        }
    }

    fn half(v: f32) -> u16 {
        // Minimal f32 -> f16 for test values (normal range only).
        let bits = v.to_bits();
        let sign = ((bits >> 16) & 0x8000) as u16;
        let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
        if v == 0.0 {
            return sign;
        }
        let mant = ((bits >> 13) & 0x3FF) as u16;
        sign | ((exp as u16) << 10) | mant
    }

    #[test]
    fn tonemaps_and_blends_on_gpu() {
        // Use the first hardware adapter; skip on machines without one (CI).
        let Ok((device, ctx)) = create_device(0) else {
            eprintln!("no GPU, skipping");
            return;
        };
        let comp = Compositor::new(&device, &ctx).unwrap();
        let target = RenderTarget::new(&device, 4, 4).unwrap();

        // FP16 source at scRGB 2.5 (= 200 nits) mid-grey-ish white.
        let v = half(2.5);
        let px: Vec<u16> = (0..16).flat_map(|_| [v, v, v, half(1.0)]).collect();
        let desc = texture_desc(
            4,
            4,
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            D3D11_BIND_SHADER_RESOURCE.0,
        );
        let init = D3D11_SUBRESOURCE_DATA {
            pSysMem: px.as_ptr().cast(),
            SysMemPitch: 4 * 8,
            SysMemSlicePitch: 0,
        };
        let mut hdr = None;
        // SAFETY: initial data matches the descriptor.
        unsafe {
            device
                .CreateTexture2D(&desc, Some(&init), Some(&mut hdr))
                .unwrap()
        };
        let hdr = hdr.unwrap();
        let srv = srv_for(&device, &hdr).unwrap();
        // 200 nits mapped to a 200-nit paper white -> 1.0 linear -> rolled off below 255.
        comp.draw(
            &target,
            &srv,
            [0.0, 0.0, 4.0, 4.0],
            DrawMode::ToneMap {
                scale: 80.0 / 200.0,
            },
        );
        let out = read_pixel(&device, &ctx, &target.texture);
        assert!(out[0] > 200 && out[0] < 255, "{out:?}");
        assert_eq!(out[0], out[1]);

        // Blend a half-transparent red overlay over it.
        let ov = Overlay::new(&device, 1, 1).unwrap();
        ov.upload(&ctx, &[0, 0, 255, 128]);
        comp.draw(
            &target,
            &ov.srv,
            [0.0, 0.0, 4.0, 4.0],
            DrawMode::Blend { opacity: 1.0 },
        );
        let out2 = read_pixel(&device, &ctx, &target.texture);
        assert!(out2[2] > out2[1], "red should dominate: {out2:?}");
    }
}
