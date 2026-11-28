//! Our own HLSL, compiled with the system's compiler, and the root signature
//! it may leave out.
//!
//! Our shaders carry their root signature (`#define RS "..."` and
//! `[RootSignature(RS)]`), and the host's root signature is made from the
//! compiled code (`CreateRootSignature(0, code)`). Microsoft's
//! d3dcompiler_47 puts it in the code (an `RTS0` part); Wine's (vkd3d-shader,
//! under Proton) leaves it out, so every such root signature failed
//! (`E_FAIL`), and with it the passes: the indirect-argument clamp (every
//! indirect draw dropped), the indirect light, the depth planes, the card
//! compares.
//!
//! [`D3DCompile`] is the system's, with the same signature: when the code it
//! returns has no `RTS0`, the entry's root signature is read from the source
//! here, serialized by the host (`D3D12SerializeVersionedRootSignature`) and
//! put into the code. Where the compiler keeps it, nothing changes.

use windows::Win32::Graphics::Direct3D::Fxc::D3DCreateBlob;
use windows::Win32::Graphics::Direct3D::{D3D_SHADER_MACRO, ID3DBlob, ID3DInclude};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{PCSTR, Param};

/// `D3DCompile`, with the root signature put back where the compiler left it
/// out (see the module).
///
/// # Safety
/// As the system's `D3DCompile`.
#[allow(non_snake_case, clippy::too_many_arguments)]
pub(crate) unsafe fn D3DCompile<P2, P5, P6>(
    psrcdata: *const core::ffi::c_void,
    srcdatasize: usize,
    psourcename: P2,
    pdefines: Option<*const D3D_SHADER_MACRO>,
    pinclude: Option<&ID3DInclude>,
    pentrypoint: P5,
    ptarget: P6,
    flags1: u32,
    flags2: u32,
    ppcode: *mut Option<ID3DBlob>,
    pperrormsgs: Option<*mut Option<ID3DBlob>>,
) -> windows::core::Result<()>
where
    P2: Param<PCSTR>,
    P5: Param<PCSTR>,
    P6: Param<PCSTR>,
{
    // SAFETY: the caller's entry name, as the system's D3DCompile takes it.
    let entry = unsafe { pentrypoint.param() }.abi();
    let target = unsafe { ptarget.param() }.abi();
    let name = unsafe { psourcename.param() }.abi();
    let compile = |source: &[u8], code: &mut Option<ID3DBlob>, errors: &mut Option<ID3DBlob>| {
        // SAFETY: as the caller's, passed through; the source lives through the call.
        unsafe {
            windows::Win32::Graphics::Direct3D::Fxc::D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                name,
                pdefines,
                pinclude,
                entry,
                target,
                flags1,
                flags2,
                code,
                Some(errors),
            )
        }
    };
    // SAFETY: the caller's source and its length.
    let source = unsafe { std::slice::from_raw_parts(psrcdata.cast::<u8>(), srcdatasize) };
    let mut errors: Option<ID3DBlob> = None;
    let mut result = compile(source, unsafe { &mut *ppcode }, &mut errors);
    // Wine's compiler aborts on the `[allow_uav_condition]` loop hint (E5017, not implemented),
    // which Microsoft's needs for loops on values read from a UAV. A hint only: compiled again
    // without it (blanked, so that the lines stay where they were).
    const HINT: &[u8] = b"[allow_uav_condition]";
    if result.is_err() && text(errors.as_ref()).contains("allow_uav_condition") && contains(source, HINT) {
        let mut plain = source.to_vec();
        let mut at = 0;
        while let Some(i) = plain[at..].windows(HINT.len()).position(|w| w == HINT) {
            plain[at + i..at + i + HINT.len()].fill(b' ');
            at += i + HINT.len();
        }
        errors = None;
        result = compile(&plain, unsafe { &mut *ppcode }, &mut errors);
    }
    if let Err(e) = &result {
        // Told here for every caller, also those that ask for no messages.
        // SAFETY: the caller's source name.
        let shader = if name.is_null() { String::new() } else { unsafe { name.to_string() }.unwrap_or_default() };
        tracing::warn!(shader, error = %e, "a shader of ours does not compile: {}", text(errors.as_ref()).trim());
    }
    if let Some(out) = pperrormsgs {
        // SAFETY: the caller's out pointer.
        unsafe { *out = errors };
    }
    result?;
    // SAFETY: the caller's entry name.
    let entry = if entry.is_null() { Vec::new() } else { unsafe { entry.as_bytes() }.to_vec() };
    // SAFETY: the out pointer the compiler just filled.
    let Some(code) = (unsafe { ppcode.as_mut() }) else { return Ok(()) };
    let Some(blob) = code.as_ref() else { return Ok(()) };
    // SAFETY: the blob's own bytes.
    let bytes = unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) };
    if part(bytes, *b"RTS0").is_some() {
        return Ok(());
    }
    let (Ok(source), Ok(entry)) = (std::str::from_utf8(source), std::str::from_utf8(&entry)) else { return Ok(()) };
    let Some(text) = root_text(source, entry) else { return Ok(()) };
    let fixed = parse(text).map_err(|e| format!("{text}: {e}")).and_then(|sig| {
        let rts0 = serialize(&sig).map_err(|e| format!("{text}: {e}"))?;
        with_part(bytes, *b"RTS0", &rts0).ok_or_else(|| "the compiled code is no DXBC container".to_owned())
    });
    match fixed {
        Ok(fixed) => {
            // SAFETY: a new blob of that size, filled whole.
            let new = unsafe { D3DCreateBlob(fixed.len())? };
            unsafe { std::ptr::copy_nonoverlapping(fixed.as_ptr(), new.GetBufferPointer().cast::<u8>(), fixed.len()) };
            *code = Some(new);
        }
        Err(e) => tracing::warn!("a root signature the compiler left out could not be put back: {e}"),
    }
    Ok(())
}

/// A blob's bytes as text (compiler messages).
fn text(blob: Option<&ID3DBlob>) -> String {
    blob.map(|b| {
        // SAFETY: the blob's own bytes.
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()) })
            .into_owned()
    })
    .unwrap_or_default()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The root signature text of `entry` in `source`: the `[RootSignature(NAME)]`
/// nearest before the entry function, and the last `#define NAME "..."`
/// before that.
fn root_text<'a>(source: &'a str, entry: &str) -> Option<&'a str> {
    let at = find_function(source, entry)?;
    let head = &source[..at];
    let attr = head.rfind("[RootSignature(")?;
    let name_start = attr + "[RootSignature(".len();
    let name = head[name_start..].split(')').next()?.trim();
    // Only the attributes of this function lie between the attribute and it.
    if head[name_start..].contains(';') || head[name_start..].contains('}') {
        return None;
    }
    let define = format!("#define {name}");
    let mut found = None;
    let mut from = 0;
    while let Some(i) = head[from..].find(&define) {
        let start = from + i + define.len();
        if head[start..].starts_with([' ', '\t']) {
            found = Some(start);
        }
        from = start;
    }
    let rest = head[found?..].trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

/// Where function `entry` is defined in `source`: its name followed by `(`,
/// after a return type.
fn find_function(source: &str, entry: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = source[from..].find(entry) {
        let at = from + i;
        let before = source[..at].chars().next_back();
        let after = source[at + entry.len()..].trim_start_matches([' ', '\t']);
        if before.is_some_and(|c| c == ' ' || c == '\t') && after.starts_with('(') {
            return Some(at);
        }
        from = at + entry.len();
    }
    None
}

/// A parsed root signature, owning what the description points at.
#[derive(Debug, Default)]
struct Sig {
    flags: D3D12_ROOT_SIGNATURE_FLAGS,
    params: Vec<(D3D12_ROOT_PARAMETER_TYPE, Kind, D3D12_SHADER_VISIBILITY)>,
    samplers: Vec<D3D12_STATIC_SAMPLER_DESC>,
}

#[derive(Debug)]
enum Kind {
    Constants(D3D12_ROOT_CONSTANTS),
    Descriptor(D3D12_ROOT_DESCRIPTOR1),
    Table(Vec<D3D12_DESCRIPTOR_RANGE1>),
}

/// Splits `text` at the commas outside parentheses, trimmed.
fn split(text: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0i32, 0);
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        out.push(last);
    }
    out
}

/// `Name(args)` as (name, args).
fn call(item: &str) -> Result<(&str, &str), String> {
    let open = item.find('(').ok_or_else(|| format!("`{item}` is not Name(...)"))?;
    let args = item[open + 1..].strip_suffix(')').ok_or_else(|| format!("`{item}` lacks its `)`"))?;
    Ok((item[..open].trim(), args))
}

/// `key = value` as (key, value), or the whole as a plain value.
fn key_value(arg: &str) -> (Option<&str>, &str) {
    match arg.split_once('=') {
        Some((k, v)) => (Some(k.trim()), v.trim()),
        None => (None, arg.trim()),
    }
}

/// A register (`b0`, `t3`, `u1`, `s0`) of kind `kind`.
fn register(value: &str, kind: char) -> Result<u32, String> {
    value
        .strip_prefix(kind)
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("`{value}` is not a {kind} register"))
}

fn number(value: &str) -> Result<u32, String> {
    value.parse().map_err(|_| format!("`{value}` is not a number"))
}

fn float(value: &str) -> Result<f32, String> {
    value.trim_end_matches('f').parse().map_err(|_| format!("`{value}` is not a number"))
}

/// Flags joined by `|`, each named by `name`; `0` is none.
fn flags(value: &str, name: impl Fn(&str) -> Option<i32>) -> Result<i32, String> {
    value.split('|').map(str::trim).try_fold(0, |all, f| {
        if f == "0" {
            Ok(all)
        } else {
            name(f).map(|v| all | v).ok_or_else(|| format!("unknown flag `{f}`"))
        }
    })
}

fn visibility(value: &str) -> Result<D3D12_SHADER_VISIBILITY, String> {
    Ok(match value {
        "SHADER_VISIBILITY_ALL" => D3D12_SHADER_VISIBILITY_ALL,
        "SHADER_VISIBILITY_VERTEX" => D3D12_SHADER_VISIBILITY_VERTEX,
        "SHADER_VISIBILITY_HULL" => D3D12_SHADER_VISIBILITY_HULL,
        "SHADER_VISIBILITY_DOMAIN" => D3D12_SHADER_VISIBILITY_DOMAIN,
        "SHADER_VISIBILITY_GEOMETRY" => D3D12_SHADER_VISIBILITY_GEOMETRY,
        "SHADER_VISIBILITY_PIXEL" => D3D12_SHADER_VISIBILITY_PIXEL,
        "SHADER_VISIBILITY_AMPLIFICATION" => D3D12_SHADER_VISIBILITY_AMPLIFICATION,
        "SHADER_VISIBILITY_MESH" => D3D12_SHADER_VISIBILITY_MESH,
        _ => return Err(format!("unknown visibility `{value}`")),
    })
}

fn root_flag(f: &str) -> Option<i32> {
    Some(
        match f {
            "ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT" => D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
            "DENY_VERTEX_SHADER_ROOT_ACCESS" => D3D12_ROOT_SIGNATURE_FLAG_DENY_VERTEX_SHADER_ROOT_ACCESS,
            "DENY_HULL_SHADER_ROOT_ACCESS" => D3D12_ROOT_SIGNATURE_FLAG_DENY_HULL_SHADER_ROOT_ACCESS,
            "DENY_DOMAIN_SHADER_ROOT_ACCESS" => D3D12_ROOT_SIGNATURE_FLAG_DENY_DOMAIN_SHADER_ROOT_ACCESS,
            "DENY_GEOMETRY_SHADER_ROOT_ACCESS" => D3D12_ROOT_SIGNATURE_FLAG_DENY_GEOMETRY_SHADER_ROOT_ACCESS,
            "DENY_PIXEL_SHADER_ROOT_ACCESS" => D3D12_ROOT_SIGNATURE_FLAG_DENY_PIXEL_SHADER_ROOT_ACCESS,
            "DENY_AMPLIFICATION_SHADER_ROOT_ACCESS" => D3D12_ROOT_SIGNATURE_FLAG_DENY_AMPLIFICATION_SHADER_ROOT_ACCESS,
            "DENY_MESH_SHADER_ROOT_ACCESS" => D3D12_ROOT_SIGNATURE_FLAG_DENY_MESH_SHADER_ROOT_ACCESS,
            "ALLOW_STREAM_OUTPUT" => D3D12_ROOT_SIGNATURE_FLAG_ALLOW_STREAM_OUTPUT,
            "LOCAL_ROOT_SIGNATURE" => D3D12_ROOT_SIGNATURE_FLAG_LOCAL_ROOT_SIGNATURE,
            "CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED" => D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED,
            "SAMPLER_HEAP_DIRECTLY_INDEXED" => D3D12_ROOT_SIGNATURE_FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED,
            _ => return None,
        }
        .0,
    )
}

fn range_flag(f: &str) -> Option<i32> {
    Some(
        match f {
            "DESCRIPTORS_VOLATILE" => D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE,
            "DATA_VOLATILE" => D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
            "DATA_STATIC_WHILE_SET_AT_EXECUTE" => D3D12_DESCRIPTOR_RANGE_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE,
            "DATA_STATIC" => D3D12_DESCRIPTOR_RANGE_FLAG_DATA_STATIC,
            "DESCRIPTORS_STATIC_KEEPING_BUFFER_BOUNDS_CHECKS" => {
                D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_STATIC_KEEPING_BUFFER_BOUNDS_CHECKS
            }
            _ => return None,
        }
        .0,
    )
}

fn descriptor_flag(f: &str) -> Option<i32> {
    Some(
        match f {
            "DATA_VOLATILE" => D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE,
            "DATA_STATIC_WHILE_SET_AT_EXECUTE" => D3D12_ROOT_DESCRIPTOR_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE,
            "DATA_STATIC" => D3D12_ROOT_DESCRIPTOR_FLAG_DATA_STATIC,
            _ => return None,
        }
        .0,
    )
}

fn filter(value: &str) -> Result<D3D12_FILTER, String> {
    Ok(match value {
        "FILTER_MIN_MAG_MIP_POINT" => D3D12_FILTER_MIN_MAG_MIP_POINT,
        "FILTER_MIN_MAG_POINT_MIP_LINEAR" => D3D12_FILTER_MIN_MAG_POINT_MIP_LINEAR,
        "FILTER_MIN_POINT_MAG_LINEAR_MIP_POINT" => D3D12_FILTER_MIN_POINT_MAG_LINEAR_MIP_POINT,
        "FILTER_MIN_POINT_MAG_MIP_LINEAR" => D3D12_FILTER_MIN_POINT_MAG_MIP_LINEAR,
        "FILTER_MIN_LINEAR_MAG_MIP_POINT" => D3D12_FILTER_MIN_LINEAR_MAG_MIP_POINT,
        "FILTER_MIN_LINEAR_MAG_POINT_MIP_LINEAR" => D3D12_FILTER_MIN_LINEAR_MAG_POINT_MIP_LINEAR,
        "FILTER_MIN_MAG_LINEAR_MIP_POINT" => D3D12_FILTER_MIN_MAG_LINEAR_MIP_POINT,
        "FILTER_MIN_MAG_MIP_LINEAR" => D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        "FILTER_ANISOTROPIC" => D3D12_FILTER_ANISOTROPIC,
        "FILTER_COMPARISON_MIN_MAG_MIP_POINT" => D3D12_FILTER_COMPARISON_MIN_MAG_MIP_POINT,
        "FILTER_COMPARISON_MIN_MAG_MIP_LINEAR" => D3D12_FILTER_COMPARISON_MIN_MAG_MIP_LINEAR,
        "FILTER_COMPARISON_MIN_MAG_LINEAR_MIP_POINT" => D3D12_FILTER_COMPARISON_MIN_MAG_LINEAR_MIP_POINT,
        "FILTER_COMPARISON_ANISOTROPIC" => D3D12_FILTER_COMPARISON_ANISOTROPIC,
        _ => return Err(format!("unknown filter `{value}`")),
    })
}

fn address(value: &str) -> Result<D3D12_TEXTURE_ADDRESS_MODE, String> {
    Ok(match value {
        "TEXTURE_ADDRESS_WRAP" => D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        "TEXTURE_ADDRESS_MIRROR" => D3D12_TEXTURE_ADDRESS_MODE_MIRROR,
        "TEXTURE_ADDRESS_CLAMP" => D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        "TEXTURE_ADDRESS_BORDER" => D3D12_TEXTURE_ADDRESS_MODE_BORDER,
        "TEXTURE_ADDRESS_MIRROR_ONCE" => D3D12_TEXTURE_ADDRESS_MODE_MIRROR_ONCE,
        _ => return Err(format!("unknown address mode `{value}`")),
    })
}

fn comparison(value: &str) -> Result<D3D12_COMPARISON_FUNC, String> {
    Ok(match value {
        "COMPARISON_NEVER" => D3D12_COMPARISON_FUNC_NEVER,
        "COMPARISON_LESS" => D3D12_COMPARISON_FUNC_LESS,
        "COMPARISON_EQUAL" => D3D12_COMPARISON_FUNC_EQUAL,
        "COMPARISON_LESS_EQUAL" => D3D12_COMPARISON_FUNC_LESS_EQUAL,
        "COMPARISON_GREATER" => D3D12_COMPARISON_FUNC_GREATER,
        "COMPARISON_NOT_EQUAL" => D3D12_COMPARISON_FUNC_NOT_EQUAL,
        "COMPARISON_GREATER_EQUAL" => D3D12_COMPARISON_FUNC_GREATER_EQUAL,
        "COMPARISON_ALWAYS" => D3D12_COMPARISON_FUNC_ALWAYS,
        _ => return Err(format!("unknown comparison `{value}`")),
    })
}

fn border(value: &str) -> Result<D3D12_STATIC_BORDER_COLOR, String> {
    Ok(match value {
        "STATIC_BORDER_COLOR_TRANSPARENT_BLACK" => D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
        "STATIC_BORDER_COLOR_OPAQUE_BLACK" => D3D12_STATIC_BORDER_COLOR_OPAQUE_BLACK,
        "STATIC_BORDER_COLOR_OPAQUE_WHITE" => D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
        _ => return Err(format!("unknown border color `{value}`")),
    })
}

/// The register letter of a descriptor kind.
fn letter(name: &str) -> Option<(char, D3D12_DESCRIPTOR_RANGE_TYPE)> {
    Some(match name {
        "CBV" => ('b', D3D12_DESCRIPTOR_RANGE_TYPE_CBV),
        "SRV" => ('t', D3D12_DESCRIPTOR_RANGE_TYPE_SRV),
        "UAV" => ('u', D3D12_DESCRIPTOR_RANGE_TYPE_UAV),
        "Sampler" => ('s', D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER),
        _ => return None,
    })
}

/// Parses a root signature as HLSL writes it, with HLSL's defaults (root
/// signature 1.1: CBVs and SRVs static while set at execute, UAVs volatile).
fn parse(text: &str) -> Result<Sig, String> {
    let mut sig = Sig::default();
    for item in split(text) {
        let (name, args) = call(item)?;
        let args = split(args);
        match name {
            "RootFlags" => {
                sig.flags = D3D12_ROOT_SIGNATURE_FLAGS(flags(args.first().copied().unwrap_or("0"), root_flag)?);
            }
            "RootConstants" => {
                let mut c = D3D12_ROOT_CONSTANTS::default();
                let mut vis = D3D12_SHADER_VISIBILITY_ALL;
                for a in args {
                    match key_value(a) {
                        (Some("num32BitConstants"), v) => c.Num32BitValues = number(v)?,
                        (Some("space"), v) => c.RegisterSpace = number(v)?,
                        (Some("visibility"), v) => vis = visibility(v)?,
                        (None, v) => c.ShaderRegister = register(v, 'b')?,
                        (Some(k), _) => return Err(format!("unknown RootConstants argument `{k}`")),
                    }
                }
                sig.params.push((D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS, Kind::Constants(c), vis));
            }
            "CBV" | "SRV" | "UAV" => {
                let (kind, ty) = match name {
                    "CBV" => ('b', D3D12_ROOT_PARAMETER_TYPE_CBV),
                    "SRV" => ('t', D3D12_ROOT_PARAMETER_TYPE_SRV),
                    _ => ('u', D3D12_ROOT_PARAMETER_TYPE_UAV),
                };
                let mut d = D3D12_ROOT_DESCRIPTOR1 {
                    Flags: if kind == 'u' {
                        D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE
                    } else {
                        D3D12_ROOT_DESCRIPTOR_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE
                    },
                    ..Default::default()
                };
                let mut vis = D3D12_SHADER_VISIBILITY_ALL;
                for a in args {
                    match key_value(a) {
                        (Some("space"), v) => d.RegisterSpace = number(v)?,
                        (Some("visibility"), v) => vis = visibility(v)?,
                        (Some("flags"), v) => d.Flags = D3D12_ROOT_DESCRIPTOR_FLAGS(flags(v, descriptor_flag)?),
                        (None, v) => d.ShaderRegister = register(v, kind)?,
                        (Some(k), _) => return Err(format!("unknown {name} argument `{k}`")),
                    }
                }
                sig.params.push((ty, Kind::Descriptor(d), vis));
            }
            "DescriptorTable" => {
                let mut ranges = Vec::new();
                let mut vis = D3D12_SHADER_VISIBILITY_ALL;
                for a in args {
                    if let (Some("visibility"), v) = key_value(a) {
                        vis = visibility(v)?;
                        continue;
                    }
                    let (kind, inner) = call(a)?;
                    let (reg, ty) = letter(kind).ok_or_else(|| format!("unknown table entry `{kind}`"))?;
                    let mut r = D3D12_DESCRIPTOR_RANGE1 {
                        RangeType: ty,
                        NumDescriptors: 1,
                        Flags: match reg {
                            'u' => D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
                            's' => D3D12_DESCRIPTOR_RANGE_FLAG_NONE,
                            _ => D3D12_DESCRIPTOR_RANGE_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE,
                        },
                        OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
                        ..Default::default()
                    };
                    for b in split(inner) {
                        match key_value(b) {
                            (Some("numDescriptors"), "unbounded") => r.NumDescriptors = u32::MAX,
                            (Some("numDescriptors"), v) => r.NumDescriptors = number(v)?,
                            (Some("space"), v) => r.RegisterSpace = number(v)?,
                            (Some("offset"), "DESCRIPTOR_RANGE_OFFSET_APPEND") => {}
                            (Some("offset"), v) => r.OffsetInDescriptorsFromTableStart = number(v)?,
                            (Some("flags"), v) => r.Flags = D3D12_DESCRIPTOR_RANGE_FLAGS(flags(v, range_flag)?),
                            (None, v) => r.BaseShaderRegister = register(v, reg)?,
                            (Some(k), _) => return Err(format!("unknown {kind} argument `{k}`")),
                        }
                    }
                    ranges.push(r);
                }
                sig.params.push((D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, Kind::Table(ranges), vis));
            }
            "StaticSampler" => {
                let mut s = D3D12_STATIC_SAMPLER_DESC {
                    Filter: D3D12_FILTER_ANISOTROPIC,
                    AddressU: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
                    AddressV: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
                    AddressW: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
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
                for a in args {
                    match key_value(a) {
                        (Some("filter"), v) => s.Filter = filter(v)?,
                        (Some("addressU"), v) => s.AddressU = address(v)?,
                        (Some("addressV"), v) => s.AddressV = address(v)?,
                        (Some("addressW"), v) => s.AddressW = address(v)?,
                        (Some("mipLODBias"), v) => s.MipLODBias = float(v)?,
                        (Some("maxAnisotropy"), v) => s.MaxAnisotropy = number(v)?,
                        (Some("comparisonFunc"), v) => s.ComparisonFunc = comparison(v)?,
                        (Some("borderColor"), v) => s.BorderColor = border(v)?,
                        (Some("minLOD"), v) => s.MinLOD = float(v)?,
                        (Some("maxLOD"), v) => s.MaxLOD = float(v)?,
                        (Some("space"), v) => s.RegisterSpace = number(v)?,
                        (Some("visibility"), v) => s.ShaderVisibility = visibility(v)?,
                        (None, v) => s.ShaderRegister = register(v, 's')?,
                        (Some(k), _) => return Err(format!("unknown StaticSampler argument `{k}`")),
                    }
                }
                sig.samplers.push(s);
            }
            _ => return Err(format!("unknown root signature entry `{name}`")),
        }
    }
    Ok(sig)
}

/// The `RTS0` part's bytes for `sig`, serialized by the host.
fn serialize(sig: &Sig) -> Result<Vec<u8>, String> {
    let params: Vec<D3D12_ROOT_PARAMETER1> = sig
        .params
        .iter()
        .map(|(ty, kind, vis)| D3D12_ROOT_PARAMETER1 {
            ParameterType: *ty,
            Anonymous: match kind {
                Kind::Constants(c) => D3D12_ROOT_PARAMETER1_0 { Constants: *c },
                Kind::Descriptor(d) => D3D12_ROOT_PARAMETER1_0 { Descriptor: *d },
                Kind::Table(r) => D3D12_ROOT_PARAMETER1_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                        NumDescriptorRanges: r.len() as u32,
                        pDescriptorRanges: r.as_ptr(),
                    },
                },
            },
            ShaderVisibility: *vis,
        })
        .collect();
    let desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        Version: windows::Win32::Graphics::Direct3D12::D3D_ROOT_SIGNATURE_VERSION_1_1,
        Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
            Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                NumStaticSamplers: sig.samplers.len() as u32,
                pStaticSamplers: sig.samplers.as_ptr(),
                Flags: sig.flags,
            },
        },
    };
    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the description and what it points at live through the call.
    let made = unsafe { D3D12SerializeVersionedRootSignature(&desc, &mut blob, Some(&mut errors)) };
    let text = |b: &ID3DBlob| unsafe {
        String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()))
            .into_owned()
    };
    if let Err(e) = made {
        return Err(format!("{e} {}", errors.as_ref().map(text).unwrap_or_default()));
    }
    let blob = blob.ok_or("no blob")?;
    // SAFETY: the blob's own bytes.
    let bytes = unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) };
    // The serialized root signature is a container holding an RTS0 part (or,
    // from some hosts, the part's bytes alone).
    Ok(part(bytes, *b"RTS0").map_or_else(|| bytes.to_vec(), <[u8]>::to_vec))
}

const HEADER: usize = 32;

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The parts of a DXBC container: (four characters, bytes).
fn parts(data: &[u8]) -> Option<Vec<([u8; 4], &[u8])>> {
    if data.get(..4)? != b"DXBC" {
        return None;
    }
    let count = u32_at(data, 28)? as usize;
    (0..count)
        .map(|i| {
            let at = u32_at(data, HEADER + 4 * i)? as usize;
            let fourcc: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
            let len = u32_at(data, at + 4)? as usize;
            Some((fourcc, data.get(at + 8..at + 8 + len)?))
        })
        .collect()
}

fn part(data: &[u8], fourcc: [u8; 4]) -> Option<&[u8]> {
    parts(data)?.into_iter().find(|(f, _)| *f == fourcc).map(|(_, d)| d)
}

/// `data`'s container with part `fourcc` added (or replaced), its size and
/// checksum made anew.
fn with_part(data: &[u8], fourcc: [u8; 4], bytes: &[u8]) -> Option<Vec<u8>> {
    let mut all: Vec<([u8; 4], &[u8])> = parts(data)?.into_iter().filter(|(f, _)| *f != fourcc).collect();
    all.push((fourcc, bytes));
    let table = HEADER + 4 * all.len();
    let total = table + all.iter().map(|(_, d)| 8 + d.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"DXBC");
    out.extend_from_slice(&[0; 16]);
    out.extend_from_slice(data.get(20..24)?);
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(all.len() as u32).to_le_bytes());
    let mut offset = table;
    for (_, d) in &all {
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += 8 + d.len();
    }
    for (f, d) in &all {
        out.extend_from_slice(f);
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(d);
    }
    let sum = checksum(&out);
    out[4..20].copy_from_slice(&sum);
    Some(out)
}

/// MD5's block function on `state`.
fn md5_block(state: &mut [u32; 4], block: &[u8; 64]) {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
        14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15,
        21, 6, 10, 15, 21,
    ];
    let m: Vec<u32> = block.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let k = ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32;
        let next = a.wrapping_add(f).wrapping_add(k).wrapping_add(m[g]).rotate_left(S[i]).wrapping_add(b);
        (a, d, c, b) = (d, c, b, next);
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d]) {
        *s = s.wrapping_add(v);
    }
}

/// The DXBC checksum of a container (bytes 4..20): MD5 of everything after
/// them, with the container format's own padding: the bit count first in the
/// last block, `(bits >> 2) | 1` last (DirectXShaderCompiler's
/// `ComputeHashRetail`, which the runtimes check).
fn checksum(data: &[u8]) -> [u8; 16] {
    let body = &data[20..];
    let mut state = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    let whole = body.len() / 64 * 64;
    for block in body[..whole].chunks_exact(64) {
        md5_block(&mut state, block.try_into().unwrap());
    }
    let bits = (body.len() as u64 * 8) as u32;
    let rest = &body[whole..];
    let mut block = [0u8; 64];
    block[..rest.len()].copy_from_slice(rest);
    let count = rest.len();
    block[count] = 0x80;
    if 64 - (count + 1) < 8 {
        md5_block(&mut state, &block);
        block[..60].fill(0);
    } else {
        block.copy_within(0..count + 1, 4);
        block[count + 5..].fill(0);
    }
    block[..4].copy_from_slice(&bits.to_le_bytes());
    block[60..].copy_from_slice(&((bits >> 2) | 1).to_le_bytes());
    md5_block(&mut state, &block);
    let mut out = [0u8; 16];
    for (o, s) in out.chunks_exact_mut(4).zip(state) {
        o.copy_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::s;

    /// MD5's own padding, to check the block function against a known digest.
    fn md5(data: &[u8]) -> [u8; 16] {
        let mut state = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
        let mut msg = data.to_vec();
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&(data.len() as u64 * 8).to_le_bytes());
        for b in msg.chunks_exact(64) {
            md5_block(&mut state, b.try_into().unwrap());
        }
        let mut out = [0u8; 16];
        for (o, s) in out.chunks_exact_mut(4).zip(state) {
            o.copy_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn the_block_function_is_md5s() {
        let hex = |d: [u8; 16]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex(md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hex(md5(&[b'a'; 1000])), "cabe45dcc9ae5b66ba86600cca6b8ba8");
    }

    #[test]
    fn root_signatures_are_read_as_hlsl_writes_them() {
        let sig = parse(
            "RootConstants(num32BitConstants=2,b0), DescriptorTable(SRV(t0,numDescriptors=4,flags=DESCRIPTORS_VOLATILE)), \
             UAV(u0), StaticSampler(s0,filter=FILTER_MIN_MAG_MIP_POINT,addressU=TEXTURE_ADDRESS_WRAP,addressV=TEXTURE_ADDRESS_CLAMP)",
        )
        .unwrap();
        assert_eq!(sig.params.len(), 3);
        let Kind::Constants(c) = &sig.params[0].1 else { panic!() };
        assert_eq!((c.Num32BitValues, c.ShaderRegister), (2, 0));
        let Kind::Table(r) = &sig.params[1].1 else { panic!() };
        assert_eq!((r[0].NumDescriptors, r[0].Flags), (4, D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE));
        let Kind::Descriptor(d) = &sig.params[2].1 else { panic!() };
        assert_eq!(d.Flags, D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE, "a UAV's default");
        assert_eq!(sig.samplers[0].AddressV, D3D12_TEXTURE_ADDRESS_MODE_CLAMP);
        assert_eq!(sig.samplers[0].AddressW, D3D12_TEXTURE_ADDRESS_MODE_WRAP, "the default");
        let sig = parse("DescriptorTable(SRV(t0, numDescriptors = 2), visibility = SHADER_VISIBILITY_PIXEL)").unwrap();
        assert_eq!(sig.params[0].2, D3D12_SHADER_VISIBILITY_PIXEL);
        assert_eq!(parse("RootFlags(0)").unwrap().params.len(), 0);
        assert!(parse("Bogus(1)").is_err());
        assert!(parse("UAV(t0)").is_err());
    }

    #[test]
    fn the_entrys_root_signature_is_found_in_the_source() {
        let src = "#define RS \"UAV(u0)\"\n#define RS2 \"SRV(t0)\"\nRWByteAddressBuffer b : register(u0);\n\
                   [RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() {}\n[RootSignature(RS2)]\nfloat4 ps(float4 p : SV_Position) : SV_Target { return p; }\n";
        assert_eq!(root_text(src, "main"), Some("UAV(u0)"));
        assert_eq!(root_text(src, "ps"), Some("SRV(t0)"));
        assert_eq!(root_text("void main() {}", "main"), None);
    }

    /// Every root signature of ours parses.
    #[test]
    fn every_root_signature_of_ours_parses() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut seen = 0;
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                for line in text.lines().filter(|l| !l.trim_start().starts_with("//")) {
                    let Some(rest) = line.split("#define RS \"").nth(1).or_else(|| line.split("#define RS \\\"").nth(1)) else {
                        continue;
                    };
                    let body = rest.split(['"', '\\']).next().unwrap();
                    parse(body).unwrap_or_else(|e| panic!("{}: {body}: {e}", p.display()));
                    seen += 1;
                }
            }
        }
        assert!(seen >= 20, "{seen}");
    }

    fn compile(source: &str, entry: PCSTR, target: PCSTR) -> Vec<u8> {
        let mut code: Option<ID3DBlob> = None;
        unsafe {
            D3DCompile(source.as_ptr().cast(), source.len(), s!("test"), None, None, entry, target, 0, 0, &mut code, None)
                .unwrap();
            let code = code.unwrap();
            std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize()).to_vec()
        }
    }

    /// Our checksum is the compiler's, on code it made.
    #[test]
    fn the_checksum_is_the_compilers() {
        let code = compile("RWByteAddressBuffer b : register(u0);\n[numthreads(1, 1, 1)]\nvoid main() { b.Store(0, 1); }\n", s!("main"), s!("cs_5_1"));
        assert_eq!(checksum(&code), code[4..20], "on {} bytes", code.len());
        let again = with_part(&code, *b"TEST", &[1, 2, 3, 4]).unwrap();
        assert_eq!(part(&again, *b"TEST"), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(checksum(&again), again[4..20]);
    }

    /// Compiles `source` here and makes its root signature and compute
    /// pipeline on the host, or tells the compiler's message.
    fn makes_on_the_host(source: &str) -> Result<(), String> {
        let Some(host) = crate::host::host() else { return Ok(()) };
        let mut code: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        unsafe {
            D3DCompile(source.as_ptr().cast(), source.len(), s!("test"), None, None, s!("main"), s!("cs_5_1"), 0, 0, &mut code, Some(&mut errors))
                .map_err(|e| format!("{e}: {}", text(errors.as_ref())))?;
            let code = code.ok_or("no code")?;
            let bytes = std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize());
            let root: ID3D12RootSignature = host.device.CreateRootSignature(0, bytes).map_err(|e| format!("root signature: {e}"))?;
            let _: ID3D12PipelineState = host
                .device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE { pShaderBytecode: bytes.as_ptr().cast(), BytecodeLength: bytes.len() },
                    ..Default::default()
                })
                .map_err(|e| format!("pipeline: {e}"))?;
        }
        Ok(())
    }

    /// The compare of copies in the card's memory (`crate::host`): without
    /// it no copy is kept in the card's memory.
    #[test]
    fn the_card_compare_is_made() {
        makes_on_the_host(crate::host::CARD_COMPARE).unwrap();
    }

    /// A shader with its root signature gives a host root signature, also
    /// under a compiler that leaves it out.
    #[test]
    fn a_root_signature_is_made_from_the_code() {
        let Some(host) = crate::host::host() else { return };
        let src = "#define RS \"RootConstants(num32BitConstants=2, b0), DescriptorTable(SRV(t0, numDescriptors=2)), UAV(u0), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR)\"\n\
                   ByteAddressBuffer a : register(t0);\nByteAddressBuffer c : register(t1);\nRWByteAddressBuffer b : register(u0);\n\
                   [RootSignature(RS)]\n[numthreads(1, 1, 1)]\nvoid main() { b.Store(0, a.Load(0) + c.Load(4)); }\n";
        let code = compile(src, s!("main"), s!("cs_5_1"));
        assert!(part(&code, *b"RTS0").is_some());
        let root: windows::core::Result<ID3D12RootSignature> = unsafe { host.device.CreateRootSignature(0, &code) };
        root.expect("a root signature from the code");
    }
}
