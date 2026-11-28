//! Two presented planes as one image: the console's display puts the second
//! (the interface) over the first (the scene), each scaled to the screen.
//! Grand Theft Auto VI presents so on Series S.

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{PCSTR, s};

use crate::host::Host;

const SOURCE: &str = r#"
#define RS "DescriptorTable(SRV(t0, numDescriptors=2), visibility=SHADER_VISIBILITY_PIXEL), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
Texture2D<float4> scene : register(t0);
Texture2D<float4> over : register(t1);
SamplerState smooth_ : register(s0);
struct V { float4 at : SV_Position; float2 uv : TEXCOORD0; };
[RootSignature(RS)]
V vs(uint id : SV_VertexID) {
    V v;
    v.uv = float2((id << 1) & 2, id & 2);
    v.at = float4(v.uv * float2(2, -2) + float2(-1, 1), 0, 1);
    return v;
}
[RootSignature(RS)]
float4 ps(V v) : SV_Target {
    float4 s = scene.SampleLevel(smooth_, v.uv, 0);
    float4 o = over.SampleLevel(smooth_, v.uv, 0);
    return float4(saturate(o.rgb + s.rgb * (1 - saturate(o.a))), 1);
}
"#;

/// [`SOURCE`] for Wine's HLSL compiler (vkd3d-shader), shader model 5.0 with
/// the root signature made in code. The same image, in plainer HLSL: no
/// integer arithmetic in the vertex shader (the corner and its texture
/// coordinate from comparisons, not shifts and masks).
///
/// The texture coordinate is interpolated over the whole output, as in
/// [`SOURCE`]. An earlier version divided the pixel's position by the base
/// plane's size instead: the base plane (the scene) is usually smaller than the
/// output, so the coordinate went past 1 over most of the screen and the
/// clamped samplers repeated one edge texel everywhere, the whole screen in one
/// colour (white).
const SOURCE_PLAIN: &str = r#"
Texture2D<float4> scene : register(t0);
Texture2D<float4> over : register(t1);
SamplerState smooth_ : register(s0);
struct V { float4 at : SV_Position; float2 uv : TEXCOORD0; };
V vs(uint id : SV_VertexID) {
    V v;
    v.uv = float2(id == 1 ? 2.0 : 0.0, id == 2 ? 2.0 : 0.0);
    v.at = float4(v.uv.x * 2.0 - 1.0, 1.0 - v.uv.y * 2.0, 0.0, 1.0);
    return v;
}
float4 ps(V v) : SV_Target {
    float2 uv = v.uv;
    float4 s = scene.SampleLevel(smooth_, uv, 0);
    float4 o = over.SampleLevel(smooth_, uv, 0);
    return float4(saturate(o.rgb + s.rgb * (1.0 - saturate(o.a))), 1.0);
}
"#;

pub(crate) struct Composer {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    views: Vec<ID3D12DescriptorHeap>,
    step: u32,
}
// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Composer {}
// SAFETY: as above.
unsafe impl Sync for Composer {}

/// `source`'s `entry` compiled for `target`; a failure carries the
/// compiler's own message.
fn compile(source: &str, entry: PCSTR, target: PCSTR) -> windows::core::Result<ID3DBlob> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; the out pointers
    // are ours.
    let compiled = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            s!("compose"),
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
    if let Err(e) = compiled {
        // SAFETY: the blob's own bytes, alive while it is.
        let message = errors.map(|b| unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                b.GetBufferPointer().cast::<u8>(),
                b.GetBufferSize(),
            ))
            .trim_end_matches('\0')
            .trim()
            .to_owned()
        });
        return Err(match message {
            Some(m) if !m.is_empty() => windows::core::Error::new(e.code(), format!("{e}: {m}")),
            _ => e,
        });
    }
    code.ok_or_else(windows::core::Error::empty)
}

/// The composer's root signature (`RS` in [`SOURCE`]) made in code: a table
/// of two SRVs (t0, t1) for the pixel shader and a linear clamp sampler at
/// s0. For hosts whose HLSL compiler leaves out `[RootSignature]`: Wine's
/// d3dcompiler_47 (vkd3d-shader), under Proton.
fn root_in_code(device: &ID3D12Device) -> windows::core::Result<ID3D12RootSignature> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 2,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
    };
    let parameter = D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                NumDescriptorRanges: 1,
                pDescriptorRanges: &range,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 16,
        ComparisonFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
        BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    };
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: 1,
        pParameters: &parameter,
        NumStaticSamplers: 1,
        pStaticSamplers: &sampler,
        Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
    };
    let mut blob: Option<ID3DBlob> = None;
    // SAFETY: the description and what it points to live through the calls;
    // the blob's bytes live while it does.
    unsafe {
        D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1_0, &mut blob, None)?;
        let blob = blob.ok_or_else(windows::core::Error::empty)?;
        device.CreateRootSignature(
            0,
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()),
        )
    }
}

impl Composer {
    /// Shader model 5.1 with the root signature inside the shaders (what
    /// Windows' compiler makes); where that fails, shader model 5.0 and the
    /// root signature made in code ([`root_in_code`]) for [`SOURCE_PLAIN`].
    pub(crate) fn new(host: &Host, format: DXGI_FORMAT) -> windows::core::Result<Self> {
        match Self::build(host, format, SOURCE, (s!("vs_5_1"), s!("ps_5_1")), false) {
            Ok(c) => Ok(c),
            Err(first) => {
                tracing::warn!(
                    error = %first,
                    "composer shaders with an embedded root signature not made; trying shader model 5.0 and a root signature made in code"
                );
                Self::build(
                    host,
                    format,
                    SOURCE_PLAIN,
                    (s!("vs_5_0"), s!("ps_5_0")),
                    true,
                )
            }
        }
    }

    fn build(
        host: &Host,
        format: DXGI_FORMAT,
        source: &str,
        (vs_target, ps_target): (PCSTR, PCSTR),
        in_code: bool,
    ) -> windows::core::Result<Self> {
        let (vs, ps) = (
            compile(source, s!("vs"), vs_target)?,
            compile(source, s!("ps"), ps_target)?,
        );
        // SAFETY: the blobs' own bytes, alive through the calls.
        unsafe {
            let bytes = |b: &ID3DBlob| {
                std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize())
            };
            let device = &host.device;
            let root: ID3D12RootSignature = if in_code {
                root_in_code(device)?
            } else {
                device.CreateRootSignature(0, bytes(&vs))?
            };
            let code = |b: &ID3DBlob| D3D12_SHADER_BYTECODE {
                pShaderBytecode: b.GetBufferPointer(),
                BytecodeLength: b.GetBufferSize(),
            };
            let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: std::mem::transmute_copy(&root),
                VS: code(&vs),
                PS: code(&ps),
                SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC {
                    FillMode: D3D12_FILL_MODE_SOLID,
                    CullMode: D3D12_CULL_MODE_NONE,
                    ..Default::default()
                },
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                ..Default::default()
            };
            desc.RTVFormats[0] = format;
            desc.BlendState.RenderTarget[0].RenderTargetWriteMask =
                D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
            let pso = device.CreateGraphicsPipelineState(&desc)?;
            Ok(Composer {
                root,
                pso,
                views: Vec::new(),
                step: device
                    .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
            })
        }
    }

    /// Each presenter command-list slot owns its own pair of SRVs. The swap
    /// chain may have several outstanding presents; the slot is selected by
    /// the completion fence, and its descriptors are only overwritten on reuse.
    pub(crate) fn record(
        &mut self,
        host: &Host,
        slot: usize,
        list: &ID3D12GraphicsCommandList,
        planes: [(&ID3D12Resource, DXGI_FORMAT); 2],
        back: &ID3D12Resource,
        (width, height): (u32, u32),
    ) -> windows::core::Result<()> {
        while self.views.len() <= slot {
            let heap = unsafe {
                host.device
                    .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                        Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                        NumDescriptors: 2,
                        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                        NodeMask: 0,
                    })?
            };
            self.views.push(heap);
        }
        let views = &self.views[slot];
        // SAFETY: an open list; the views are made in slots no frame in flight
        // reads; live textures.
        unsafe {
            let (cpu, gpu) = (
                views.GetCPUDescriptorHandleForHeapStart(),
                views.GetGPUDescriptorHandleForHeapStart(),
            );
            for (i, plane) in planes.iter().enumerate() {
                let at = D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: cpu.ptr + (i * self.step as usize),
                };
                let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: plane.1,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_SRV {
                            MipLevels: 1,
                            ..Default::default()
                        },
                    },
                };
                host.device
                    .CreateShaderResourceView(Some(plane.0), Some(&desc), at);
            }
            let rtv = host.rtv(Some(back), None);
            list.OMSetRenderTargets(1, Some(&rtv), false, None);
            list.SetDescriptorHeaps(&[Some(views.clone())]);
            list.SetGraphicsRootSignature(&self.root);
            list.SetPipelineState(&self.pso);
            list.SetGraphicsRootDescriptorTable(0, gpu);
            list.IASetPrimitiveTopology(
                windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
            );
            list.RSSetViewports(&[D3D12_VIEWPORT {
                Width: width as f32,
                Height: height as f32,
                MaxDepth: 1.0,
                ..Default::default()
            }]);
            list.RSSetScissorRects(&[windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            }]);
            list.DrawInstanced(3, 1, 0, 0);
        }
        Ok(())
    }
}

/// Select a legal float-sampled view of a display plane, preserving a typed
/// resource's format. Typeless resources use the corresponding display type.
pub(crate) fn view(d: &D3D12_RESOURCE_DESC) -> Option<DXGI_FORMAT> {
    if d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || d.DepthOrArraySize != 1
        || d.SampleDesc.Count != 1
        || d.Flags.contains(D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE)
    {
        return None;
    }
    match d.Format.0 {
        9 => Some(DXGI_FORMAT_R16G16B16A16_FLOAT),
        23 => Some(DXGI_FORMAT_R10G10B10A2_UNORM),
        27 => Some(DXGI_FORMAT_R8G8B8A8_UNORM),
        90 => Some(DXGI_FORMAT_B8G8R8A8_UNORM),
        10 | 11 | 13 | 24 | 26 | 28 | 29 | 31 | 67 | 87 | 91 => Some(d.Format),
        _ => None,
    }
}

#[cfg(all(test, feature = "diagnostics"))]
mod tests {
    use super::*;

    #[test]
    fn composes_scaled_premultiplied_plane_on_gpu() {
        let Some(host) = crate::host::host() else {
            eprintln!("skipping display-plane readback: no D3D12 device");
            return;
        };
        let format = DXGI_FORMAT_R8G8B8A8_UNORM;
        composes(host, Composer::new(host, format).unwrap(), format);
    }

    /// The fallback Wine's compiler gets ([`SOURCE_PLAIN`]), made on purpose so
    /// that it is checked where the first way works too. The base plane is
    /// smaller than the output, as the scene is in a title: the image must not
    /// shift, nor turn into the colour of one edge texel.
    #[test]
    fn the_wine_fallback_composes_the_same_image() {
        let Some(host) = crate::host::host() else {
            eprintln!("skipping display-plane readback: no D3D12 device");
            return;
        };
        let format = DXGI_FORMAT_R8G8B8A8_UNORM;
        let plain = Composer::build(host, format, SOURCE_PLAIN, (s!("vs_5_0"), s!("ps_5_0")), true)
            .unwrap();
        composes(host, plain, format);
    }

    fn composes(host: &Host, mut compositor: Composer, format: DXGI_FORMAT) {
        let list = host.new_list().unwrap();
        let texture = |width: u64| -> ID3D12Resource {
            let mut resource = None;
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: width,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                ..Default::default()
            };
            unsafe {
                host.device
                    .CreateCommittedResource(
                        &D3D12_HEAP_PROPERTIES {
                            Type: D3D12_HEAP_TYPE_DEFAULT,
                            ..Default::default()
                        },
                        D3D12_HEAP_FLAG_NONE,
                        &desc,
                        D3D12_RESOURCE_STATE_RENDER_TARGET,
                        None,
                        &mut resource,
                    )
                    .unwrap()
            };
            resource.unwrap()
        };
        // A half-red base at one resolution. The overlay at output resolution
        // contains transparent, half-opaque green and fully opaque blue pixels.
        let base = texture(1);
        let overlay = texture(3);
        let output = texture(3);
        let target = D3D12_RESOURCE_STATE_RENDER_TARGET;
        let read = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
        let cmds = &list.list;
        unsafe {
            cmds.ClearRenderTargetView(host.rtv(Some(&base), None), &[0.5, 0.0, 0.0, 1.0], None);
            let overlay_rtv = host.rtv(Some(&overlay), None);
            let colors = [
                [0.0, 0.0, 0.0, 0.0],
                [0.0, 0.5, 0.0, 0.5],
                [0.0, 0.0, 1.0, 1.0],
            ];
            for (i, color) in colors.iter().enumerate() {
                let rect = windows::Win32::Foundation::RECT {
                    left: i as i32,
                    right: i as i32 + 1,
                    top: 0,
                    bottom: 1,
                };
                cmds.ClearRenderTargetView(overlay_rtv, color, Some(&[rect]));
            }
            cmds.ResourceBarrier(&[
                crate::host::transition(&base, target, read),
                crate::host::transition(&overlay, target, read),
            ]);
            compositor
                .record(
                    host,
                    0,
                    cmds,
                    [(&base, format), (&overlay, format)],
                    &output,
                    (3, 1),
                )
                .unwrap();
            cmds.ResourceBarrier(&[crate::host::transition(
                &output,
                target,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            let (buffer, footprint) =
                crate::capture::read_back(&host.device, cmds, &output).unwrap();
            list.close();
            host.execute(&[&list]);
            host.device.GetDeviceRemovedReason().unwrap();
            let mut mapped = std::ptr::null_mut();
            buffer.Map(0, None, Some(&mut mapped)).unwrap();
            let start = mapped.cast::<u8>().add(footprint.Offset as usize);
            let pixels: [u8; 12] = std::slice::from_raw_parts(start, 12).try_into().unwrap();
            buffer.Unmap(0, None);
            for (actual, expected) in pixels
                .iter()
                .zip([128, 0, 0, 255, 64, 128, 0, 255, 0, 0, 255, 255])
            {
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "unexpected composite {pixels:?}"
                );
            }
            eprintln!("D3D12 display-plane readback RGBA: {pixels:?}");
        }
    }
}
