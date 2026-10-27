//! Startup-gated removal of redundant host objects and repeat-only copies.
//! No GPU contents, encoders, submission order or synchronization live here.
use std::{collections::{HashMap, VecDeque}, hash::Hash, sync::OnceLock};

pub const VIEW_LIMIT: usize = 256;
pub const TEXTURE_BYTE_LIMIT: u64 = 32 << 20;

pub fn requested() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MUGUET_NATIVE_RENDER_FASTPATH").is_some_and(|v| v == "1"))
}

pub fn cache_repeats(enabled: bool, unlimited_native: bool) -> bool {
    !enabled || !unlimited_native
}

/// OFF eagerly allocates the original fallback. ON allocates it only if the
/// completed output graph has no SR binding. Native required-SR errors exit
/// before finish_binding, so this cannot turn an SR error into a raw fallback.
pub fn initial_binding<T>(enabled: bool, create: impl FnOnce() -> T) -> Option<T> {
    if enabled { None } else { Some(create()) }
}
pub fn finish_binding<T>(binding: Option<T>, create: impl FnOnce() -> T) -> T {
    binding.unwrap_or_else(create)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ProxyKind { Depth, Stencil, FloatDepth, Half }

/// Every semantic field of wgpu30's view descriptor after normalization.
/// A debug label is deliberately excluded. Texture identity is an owning
/// public WGPU handle, never a recycled raw VkImage/VkImageView integer.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ProxyViewKey<I, F, D, A> {
    pub resource: u32,
    pub srv: u32,
    pub texture: I,
    pub texture_bytes: u64,
    pub kind: ProxyKind,
    pub format: F,
    pub dimension: D,
    pub usage: u32,
    pub aspect: A,
    pub mip: u32,
    pub mips: u32,
    pub layer: u32,
    pub layers: u32,
}

/// Logical bytes of all D2 mip/layer data retained by one proxy identity.
/// Alignment/compression/private driver allocation overhead is not estimated.
pub fn texture_bytes(width: u32, height: u32, layers: u32, mips: u32, bytes_per_pixel: u64) -> Option<u64> {
    if width == 0 || height == 0 || layers == 0 || mips == 0 || mips > 32 || bytes_per_pixel == 0 { return None; }
    let mut total=0u64;
    for mip in 0..mips {
        let pixels=u64::from((width>>mip).max(1)).checked_mul(u64::from((height>>mip).max(1)))?;
        total=total.checked_add(pixels.checked_mul(u64::from(layers))?.checked_mul(bytes_per_pixel)?)?;
    }
    Some(total)
}

pub trait ImageKey {
    type Image: Eq + Hash + Clone;
    fn image(&self) -> &Self::Image;
    fn bytes(&self) -> u64;
}
impl<I: Eq + Hash + Clone, F, D, A> ImageKey for ProxyViewKey<I,F,D,A> {
    type Image=I;
    fn image(&self)->&I { &self.texture }
    fn bytes(&self)->u64 { self.texture_bytes }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats { pub hits: u64, pub misses: u64, pub evictions: u64, pub invalidated: u64, pub rejected: u64 }

/// Entry and unique-image-byte bounds apply globally. Repeated views of one
/// Texture count its mip/layer bytes once, and destruction removes its keys.
pub struct ViewCache<K: ImageKey, V> {
    entries: HashMap<K, V>,
    order: VecDeque<K>,
    images: HashMap<K::Image,(u64,usize)>,
    bytes: u64,
    limit: usize,
    byte_limit: u64,
    stats: Stats,
}
impl<K: ImageKey + Eq + Hash + Clone, V: Clone> ViewCache<K, V> {
    pub fn new(limit: usize) -> Self { Self::with_limits(limit,TEXTURE_BYTE_LIMIT) }
    pub fn with_limits(limit: usize, byte_limit: u64) -> Self {
        Self { entries:HashMap::new(),order:VecDeque::new(),images:HashMap::new(),bytes:0,limit,byte_limit,stats:Stats::default() }
    }
    pub fn get(&mut self, key: &K) -> Option<V> {
        let value = self.entries.get(key).cloned();
        if value.is_some() { self.stats.hits += 1; } else { self.stats.misses += 1; }
        value
    }
    fn remove(&mut self,key:&K) {
        if self.entries.remove(key).is_none() { return; }
        let image=key.image();
        if let Some((bytes,refs))=self.images.get_mut(image) {
            *refs-=1;
            if *refs==0 { self.bytes-=*bytes;self.images.remove(image); }
        }
    }
    pub fn insert(&mut self, key: K, value: V) {
        let bytes=key.bytes();
        if self.limit==0 || bytes==0 || bytes>self.byte_limit
            || self.images.get(key.image()).is_some_and(|&(stored,_)|stored!=bytes) {
            self.stats.rejected+=1;return;
        }
        if let Some(old)=self.entries.get_mut(&key) { *old=value;return; }
        loop {
            let additional=if self.images.contains_key(key.image()){0}else{bytes};
            if self.entries.len()<self.limit && self.bytes.checked_add(additional).is_some_and(|n|n<=self.byte_limit) {break;}
            if let Some(old)=self.order.pop_front() {self.remove(&old);self.stats.evictions+=1;}
        }
        self.images.entry(key.image().clone()).and_modify(|(_,refs)|*refs+=1).or_insert_with(||{self.bytes+=bytes;(bytes,1)});
        self.order.push_back(key.clone());self.entries.insert(key,value);
    }
    pub fn invalidate(&mut self, mut remove: impl FnMut(&K) -> bool) {
        let retired:Vec<_>=self.entries.keys().filter(|key|remove(key)).cloned().collect();
        for key in &retired {self.remove(key);}
        self.order.retain(|key|self.entries.contains_key(key));self.stats.invalidated+=retired.len() as u64;
    }
    pub fn clear(&mut self) {
        self.stats.invalidated+=self.entries.len() as u64;self.entries.clear();self.order.clear();self.images.clear();self.bytes=0;
    }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn bytes(&self) -> u64 { self.bytes }
    pub fn stats(&self) -> Stats { self.stats }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};
    type Key = ProxyViewKey<u64, u32, u32, u32>;
    fn key() -> Key { Key { resource:7, srv:9, texture:100, texture_bytes:4, kind:ProxyKind::Depth, format:1, dimension:2, usage:4, aspect:0, mip:0, mips:1, layer:0, layers:1 } }
    #[test] fn flag_off_and_legacy_keep_repeat_cache() {
        assert!(cache_repeats(false, false)); assert!(cache_repeats(false, true));
        assert!(cache_repeats(true, false)); assert!(!cache_repeats(true, true));
    }
    #[test] fn successful_sr_only_allocates_legacy_fallback_when_flag_off() {
        for enabled in [false, true] {
            let calls=Cell::new(0);
            let original=initial_binding(enabled, || {calls.set(calls.get()+1); "raw"});
            assert_eq!(calls.get(),u32::from(!enabled));
            let _ = original;
            let selected=finish_binding(Some("same-SR-pixels"), || {calls.set(calls.get()+1); "raw"});
            assert_eq!(selected,"same-SR-pixels"); assert_eq!(calls.get(),u32::from(!enabled));
        }
    }
    #[test] fn no_sr_and_permitted_fallback_create_exactly_one_identical_binding() {
        for enabled in [false, true] {
            let calls=Cell::new(0);
            let original=initial_binding(enabled, || {calls.set(calls.get()+1); "raw"});
            let selected=finish_binding(original, || {calls.set(calls.get()+1); "raw"});
            assert_eq!(selected,"raw"); assert_eq!(calls.get(),1);
        }
    }
    #[test] fn required_sr_error_can_exit_without_constructing_raw_binding() {
        let calls=Cell::new(0);
        let pending=initial_binding(true, || {calls.set(calls.get()+1); "raw"});
        assert!(pending.is_none()); let _ = pending; assert_eq!(calls.get(),0);
    }
    #[test] fn full_identity_and_every_descriptor_field_are_distinct() {
        let a=key(); let mut cache=ViewCache::new(32); cache.insert(a.clone(),"depth");
        let mut alternatives=Vec::new();
        macro_rules! different {($field:ident,$value:expr)=>{{let mut k=a.clone();k.$field=$value;alternatives.push(k);}}}
        different!(resource,8); different!(srv,10); different!(texture,101);
        for kind in [ProxyKind::Stencil,ProxyKind::FloatDepth,ProxyKind::Half] {different!(kind,kind);}
        different!(format,2); different!(dimension,3); different!(usage,8); different!(aspect,1);
        different!(mip,1); different!(mips,2); different!(layer,1); different!(layers,2);
        for k in alternatives {assert!(cache.get(&k).is_none());cache.insert(k,"other");}
        assert_eq!(cache.get(&a),Some("depth"));
    }
    #[test] fn dirty_content_updates_share_view_without_caching_pixels() {
        let image=Rc::new(Cell::new(11));let mut cache=ViewCache::new(4);cache.insert(key(),image.clone());
        let held=cache.get(&key()).unwrap();image.set(27);assert_eq!(held.get(),27);
        assert!(Rc::ptr_eq(&held,&cache.get(&key()).unwrap()));assert_eq!(cache.stats().hits,2);
    }
    #[test] fn same_key_update_does_not_duplicate_fifo_or_exceed_limit() {
        let mut cache=ViewCache::new(2);let a=key();let mut b=a.clone();b.srv=11;let mut c=a.clone();c.srv=12;
        cache.insert(a.clone(),1);cache.insert(a.clone(),2);cache.insert(b.clone(),3);cache.insert(c.clone(),4);
        assert_eq!(cache.len(),2);assert_eq!(cache.get(&a),None);assert_eq!(cache.get(&b),Some(3));
        assert_eq!(cache.get(&c),Some(4));assert_eq!(cache.stats().evictions,1);
    }
    #[test] fn global_capacity_survives_many_distinct_descriptors() {
        let mut cache=ViewCache::new(VIEW_LIMIT);
        for layer in 0..4096 {let mut k=key();k.layer=layer;cache.insert(k,layer);assert!(cache.len()<=VIEW_LIMIT);}
        assert_eq!(cache.len(),VIEW_LIMIT);assert_eq!(cache.stats().evictions,4096-VIEW_LIMIT as u64);
    }
    #[test] fn invalidating_resource_or_srv_releases_views_but_not_inflight_clones() {
        let mut cache=ViewCache::new(8);let a=key();let mut b=a.clone();b.resource=8;
        let image=Rc::new(());let weak=Rc::downgrade(&image);cache.insert(a.clone(),image.clone());
        let inflight=cache.get(&a).unwrap();drop(image);cache.insert(b.clone(),Rc::new(()));
        cache.invalidate(|k:&Key|k.resource==7||k.srv==77);assert_eq!(cache.len(),1);assert!(weak.upgrade().is_some());
        drop(inflight);assert!(weak.upgrade().is_none());assert_eq!(cache.get(&b).is_some(),true);
        cache.invalidate(|k:&Key|k.srv==9);assert_eq!(cache.len(),0);assert_eq!(cache.stats().invalidated,2);
    }
    #[test] fn error_clear_cannot_leave_retained_fifo_identities_or_stale_hits() {
        let mut cache=ViewCache::new(2);cache.insert(key(),Rc::new(()));cache.clear();
        assert!(cache.order.is_empty());assert_eq!(cache.len(),0);assert!(cache.get(&key()).is_none());
        cache.insert(key(),Rc::new(()));assert_eq!(cache.len(),1);
    }
    #[test] fn zero_capacity_retains_nothing() {let mut cache=ViewCache::new(0);cache.insert(key(),1);assert_eq!(cache.len(),0);}

    #[test] fn mip_layer_bytes_and_overflow_are_checked() {
        assert_eq!(texture_bytes(1200,800,1,1,4),Some(3_840_000));
        assert_eq!(texture_bytes(4,2,3,3,4),Some((8+2+1)*3*4));
        assert_eq!(texture_bytes(u32::MAX,u32::MAX,u32::MAX,32,8),None);
        assert_eq!(texture_bytes(1,1,1,33,4),None);assert_eq!(texture_bytes(1,1,0,1,4),None);
    }
    #[test] fn unique_texture_bytes_are_counted_once_across_views_and_evicted_exactly() {
        let mut cache=ViewCache::with_limits(8,8);let a=key();let mut b=a.clone();b.srv=10;
        cache.insert(a.clone(),1);cache.insert(b.clone(),2);assert_eq!(cache.bytes(),4);
        let mut c=a.clone();c.texture=200;cache.insert(c.clone(),3);assert_eq!(cache.bytes(),8);
        cache.invalidate(|k:&Key|k.srv==9);assert_eq!(cache.bytes(),4);assert_eq!(cache.len(),1);
        cache.clear();assert_eq!(cache.bytes(),0);assert!(cache.images.is_empty());
    }
    #[test] fn byte_limit_evicts_last_identity_reference_and_oversize_bypasses() {
        let mut cache=ViewCache::with_limits(256,4);let a=key();let mut b=a.clone();b.texture=200;
        cache.insert(a.clone(),1);cache.insert(b.clone(),2);assert!(cache.get(&a).is_none());assert_eq!(cache.bytes(),4);
        let mut big=b.clone();big.texture_bytes=5;cache.insert(big.clone(),3);assert!(cache.get(&big).is_none());
        assert_eq!(cache.get(&b),Some(2));assert_eq!(cache.stats().rejected,1);
    }
    #[test] fn inconsistent_identity_size_does_not_undercharge_or_replace_existing() {
        let mut cache=ViewCache::with_limits(8,32);let a=key();cache.insert(a.clone(),1);
        let mut bad=a.clone();bad.srv+=1;bad.texture_bytes=8;cache.insert(bad.clone(),2);
        assert!(cache.get(&bad).is_none());assert_eq!(cache.get(&a),Some(1));assert_eq!(cache.bytes(),4);
    }
}
