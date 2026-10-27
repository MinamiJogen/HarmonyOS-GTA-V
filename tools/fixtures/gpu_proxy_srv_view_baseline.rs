// Frozen production srv_view from commit e265c1f5807533dbc3817fc020758c8b7b9f4574.
// Only rename and model-compatible None range arguments changed.
    fn legacy_srv_view(&mut self, srv_id: u32, b: &Binding) -> Option<TextureView> {
        let (res, fmt, a, bb, c, d) = { let v = self.view_obj(srv_id)?; (v.res, v.fmt, v.a, v.b, v.c, v.d) };
        if self.astc_pending.contains(&res) { self.flush_astc(); }
        let t = self.tex(res)?;
        let (tformat, desc, astc) = (t.format, t.desc, t.astc);
        let (a, bb) = if desc.skip == 0 || bb == 0xffff_ffff || bb == 0 { (a.saturating_sub(desc.skip), bb) } else { (a.saturating_sub(desc.skip), bb.saturating_sub(desc.skip.saturating_sub(a)).max(1)) };
        let mut proxied = false;
        let (tex, format) = if is_depth(tformat) && b.sample != Sample::Depth {
            proxied = true;
            self.depth_proxy(res, b.sample == Sample::Uint, None)?
        } else if !is_depth(tformat) && b.sample == Sample::Depth {
            proxied = true;
            self.float_to_depth_proxy(res, None)?
        } else if self.needs_half(tformat, b) && desc.dim != TextureDimension::D3 && desc.samples == 1 {
            proxied = true;
            self.half_proxy(res, None)?
        } else {
            (self.tex(res)?.tex.clone(), tformat)
        };
        let key = format!("{:?}{}", b.dim, if proxied { ":p" } else { "" });
        if let Some(v) = self.view_obj(srv_id).and_then(|v| v.views.get(&key)) { return Some(v.clone()); }
        let total = desc.mips;
        let mip_count = if bb == 0xffff_ffff || bb == 0 { total.saturating_sub(a) } else { bb };
        let layers = desc.d;
        if matches!(b.dim, TextureViewDimension::Cube | TextureViewDimension::CubeArray) && layers.saturating_sub(c) < 6 {
            log_once(&format!("cubeview{res}"), &format!("cube SRV over a texture with {layers} layer(s): bound as an empty cube"));
            return None;
        }
        let mut vd = TextureViewDescriptor {
            dimension: Some(b.dim),
            base_mip_level: a.min(total - 1),
            mip_level_count: Some(mip_count.min(total.saturating_sub(a)).max(1)),
            ..Default::default()
        };
        if matches!(b.dim, TextureViewDimension::D2Array | TextureViewDimension::Cube | TextureViewDimension::CubeArray | TextureViewDimension::D2) {
            let cnt = match b.dim { TextureViewDimension::Cube => 6, TextureViewDimension::D2 => 1, _ => if d != 0 { d } else { layers - c } };
            vd.base_array_layer = c.min(layers - 1);
            vd.array_layer_count = Some(cnt.min(layers.saturating_sub(c)).max(1));
        }
        if fmt != 0 && !is_depth(format) && !proxied {
            let mf = map_format(fmt, 0).format;
            if mf != format && mf.is_srgb() != format.is_srgb() && mf.remove_srgb_suffix() == format.remove_srgb_suffix() { vd.format = Some(mf); }
            if astc && mf.is_srgb() != format.is_srgb() {
                vd.format = Some(TextureFormat::Astc { block: AstcBlock::B4x4, channel: if mf.is_srgb() { AstcChannel::UnormSrgb } else { AstcChannel::Unorm } });
            }
        }
        if is_depth(format) && !proxied {
            vd.aspect = if fmt == 47 || fmt == 22 { TextureAspect::StencilOnly } else { TextureAspect::DepthOnly };
        }
        let v = self.scoped("SRV view", |_| tex.create_view(&vd))?;
        if !proxied {
            if let Some(vo) = self.view_obj_mut(srv_id) { vo.views.insert(key, v.clone()); }
        }
        Some(v)
    }
