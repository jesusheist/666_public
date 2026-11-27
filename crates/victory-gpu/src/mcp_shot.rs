//! The debug server's screenshot (`VICTORY_MCP`): the present path copies
//! the frame into a readback buffer, runs and waits for that list, and hands
//! the rows over; the server's thread converts and writes the picture.

use victory_mcp::{RawFrame, ShotRequest, shot};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

/// A frame being copied into a readback buffer.
pub(crate) struct Pending {
    buffer: ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    request: ShotRequest,
}

/// Records the copy of `texture` (the base plane, or the composed back buffer;
/// in the copy-source state) into a readback
/// buffer on the open `list`. The request is answered with the reason if
/// that cannot be done.
pub(crate) fn record(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    texture: &ID3D12Resource,
    request: ShotRequest,
) -> Option<Pending> {
    // SAFETY: a live texture.
    let desc = unsafe { texture.GetDesc() };
    if shot::bytes_per_pixel(desc.Format.0 as u32).is_none() {
        request.fail(format!("the frame's pixel format ({}) is not converted", desc.Format.0));
        return None;
    }
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total = 0;
    // SAFETY: outputs for one subresource.
    unsafe {
        device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), None, None, Some(&mut total));
    }
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    let buffer_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: total,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    // SAFETY: plain creation; an open list; locations naming live resources.
    let made = unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &buffer_desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut buffer,
        )
    };
    let Some(buffer) = buffer.filter(|_| made.is_ok()) else {
        request.fail(format!("no readback buffer of {total} bytes: {made:?}"));
        return None;
    };
    // SAFETY: the locations name live resources and an open list.
    unsafe {
        let to = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&buffer),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(texture),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
    }
    Some(Pending { buffer, footprint, request })
}

impl Pending {
    /// Hands the rows over once the list has run: the pixels without the
    /// padding at the end of each row.
    pub(crate) fn deliver(self, frame: u64) {
        let f = self.footprint.Footprint;
        let format = f.Format.0 as u32;
        let Some(bpp) = shot::bytes_per_pixel(format) else {
            self.request.fail(format!("the frame's pixel format ({format}) is not converted"));
            return;
        };
        let (width, height, pitch, row) = (f.Width as usize, f.Height as usize, f.RowPitch as usize, f.Width as usize * bpp);
        let mut data = std::ptr::null_mut();
        // SAFETY: the copy has finished (the present list was run and waited
        // for); the mapping spans the footprint's rows, and the reads stay
        // inside `pitch * (height - 1) + row` bytes of it.
        let rows = unsafe {
            if self.buffer.Map(0, None, Some(&mut data)).is_err() || data.is_null() {
                self.request.fail("the readback buffer could not be mapped");
                return;
            }
            let bytes = std::slice::from_raw_parts(data.cast::<u8>(), pitch * height.saturating_sub(1) + row);
            let mut rows = Vec::with_capacity(row * height);
            for y in 0..height {
                rows.extend_from_slice(&bytes[y * pitch..y * pitch + row]);
            }
            self.buffer.Unmap(0, None);
            rows
        };
        self.request.deliver(RawFrame { width: width as u32, height: height as u32, format, frame, rows });
    }
}
