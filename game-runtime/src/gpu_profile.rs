//! Sparse, bounded timestamps. Query results only advance through existing wgpu polls.
//! Frame spans cover recorded command work, excluding pending queue.write_* uploads.
use std::{io::Write, path::PathBuf, sync::{Arc, atomic::{AtomicU8, Ordering}}, time::Instant};
use wgpu::*;
#[path = "gpu_profile_model.rs"]
mod model;
use model::*;

pub fn requested() -> bool { std::env::var_os("MUGUET_GPU_PROFILE").is_some_and(|v| v == "1") }
pub fn features(adapter: &Adapter) -> Features {
    let required = Features::TIMESTAMP_QUERY | Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    if requested() && adapter.features().contains(required) { required } else { Features::empty() }
}

pub struct Pair { query: QuerySet, begin: u32, end: u32 }
impl Pair {
    pub fn render(&self) -> RenderPassTimestampWrites<'_> { RenderPassTimestampWrites { query_set: &self.query, beginning_of_pass_write_index: Some(self.begin), end_of_pass_write_index: Some(self.end) } }
    pub fn compute(&self) -> ComputePassTimestampWrites<'_> { ComputePassTimestampWrites { query_set: &self.query, beginning_of_pass_write_index: Some(self.begin), end_of_pass_write_index: Some(self.end) } }
    pub fn begin(&self, enc: &mut CommandEncoder) { enc.write_timestamp(&self.query, self.begin); }
    pub fn end(&self, enc: &mut CommandEncoder) { enc.write_timestamp(&self.query, self.end); }
}
struct Record { label_truncated: bool, kind: &'static str, label: String, begin: u32, end: u32 }
#[derive(PartialEq)]
enum State { Idle, Recording, Ready, Mapping }
struct Slot {
    query: QuerySet, resolve: Buffer, read: Buffer, mapped: Arc<AtomicU8>, state: State,
    frame: u64, next: u32, began: bool, records: Vec<Record>, truncated: u64, started: Instant,
}
pub struct Profiler {
    slots: Vec<Slot>, active: Option<usize>, valid_bits: u32, period: f32, dir: PathBuf,
    bytes: u64, output_truncated: bool, dropped_busy: u64, map_errors: u64, io_errors: u64, reports: u64,
}
impl Profiler {
    pub fn new(device: &Device, queue: &Queue) -> Option<Self> {
        if !requested() { return None; }
        let dir = PathBuf::from(std::env::var_os("MUGUET_GPU_PROFILE_DIR")?);
        let required = Features::TIMESTAMP_QUERY | Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let mut family = 0; let mut valid_bits = 0;
        #[cfg(target_env = "ohos")]
        if let Some(hal) = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() } {
            (family, valid_bits, _) = hal.timestamp_query_capabilities();
        }
        let period = queue.get_timestamp_period();
        let supported = device.features().contains(required) && (36..=64).contains(&valid_bits) && period.is_finite() && period > 0.0;
        let _ = std::fs::create_dir_all(&dir);
        let capabilities = serde_json::json!({"processId":std::process::id(), "supported":supported,
            "queueFamily":family,"timestampValidBits":valid_bits,"timestampPeriodNs":period,
            "requestedFeatures":format!("{required:?}"),"enabledFeatures":format!("{:?}",device.features()),
            "sampleEveryFrames":SAMPLE_EVERY,"slots":SLOTS,"queryCapacityPerFrame":QUERIES,
            "maxOutputBytes":MAX_OUTPUT_BYTES,"extraSubmits":0,"extraWaits":0,"extraPolls":0,
            "scope":"recorded-command frame span; excludes pending queue.write uploads, not GPU busy time",
            "timestampStage":"wgpu Vulkan BOTTOM_OF_PIPE; stage overlap and command reordering affect attribution"});
        if let Ok(mut f) = std::fs::File::create(dir.join("capabilities.json")) { let _ = writeln!(f,"{capabilities}"); let _ = f.sync_all(); }
        if !supported { return None; }
        // A cold process starts a fresh bounded ledger. Every record also carries its PID.
        let _ = std::fs::File::create(dir.join("frames.jsonl"));
        let _ = std::fs::remove_file(dir.join("truncated.json"));
        let slots = (0..SLOTS).map(|_| Slot {
            query:device.create_query_set(&QuerySetDescriptor {label:Some("bounded GPU profile"),ty:QueryType::Timestamp,count:QUERIES}),
            resolve:device.create_buffer(&BufferDescriptor {label:Some("GPU profile resolve"),size:QUERIES as u64*8,usage:BufferUsages::QUERY_RESOLVE|BufferUsages::COPY_SRC,mapped_at_creation:false}),
            read:device.create_buffer(&BufferDescriptor {label:Some("GPU profile readback"),size:QUERIES as u64*8,usage:BufferUsages::COPY_DST|BufferUsages::MAP_READ,mapped_at_creation:false}),
            mapped:Arc::new(AtomicU8::new(0)),state:State::Idle,frame:0,next:2,began:false,records:Vec::new(),truncated:0,started:Instant::now(),
        }).collect();
        let mut p = Self {slots,active:None,valid_bits,period,dir,bytes:0,output_truncated:false,dropped_busy:0,map_errors:0,io_errors:0,reports:0};
        p.begin_frame(0); Some(p)
    }
    pub fn begin_frame(&mut self, frame: u64) {
        self.collect();
        if frame % SAMPLE_EVERY != 0 || self.output_truncated { return; }
        debug_assert!(self.active.is_none());
        let Some(i) = self.slots.iter().position(|s| s.state == State::Idle) else { self.dropped_busy+=1; return; };
        let s = &mut self.slots[i];
        s.state=State::Recording; s.frame=frame; s.next=2; s.began=false; s.records.clear(); s.truncated=0; s.started=Instant::now();
        self.active=Some(i);
    }
    pub fn encoder_begin(&mut self, enc: &mut CommandEncoder) {
        if let Some(i)=self.active { let s=&mut self.slots[i]; if !s.began { enc.write_timestamp(&s.query,0); s.began=true; } }
    }
    pub fn pair(&mut self, kind: &'static str, label: impl FnOnce()->String) -> Option<Pair> {
        let s=&mut self.slots[self.active?];
        let Some((begin,end))=reserve_pair(&mut s.next) else { s.truncated+=1; return None; };
        let label=label();
        s.records.push(Record {kind,label_truncated:label.chars().count()>384,label:label.chars().take(384).collect(),begin,end});
        Some(Pair {query:s.query.clone(),begin,end})
    }
    pub fn cancel_empty_frame(&mut self) {
        if let Some(i)=self.active.take() { self.slots[i].state=State::Idle; }
    }
    pub fn finish_frame(&mut self, enc: &mut CommandEncoder) {
        let Some(i)=self.active.take() else { return; };
        let s=&mut self.slots[i];
        if !s.began { s.state=State::Idle; return; }
        enc.write_timestamp(&s.query,1);
        enc.resolve_query_set(&s.query,0..s.next,&s.resolve,0);
        enc.copy_buffer_to_buffer(&s.resolve,0,&s.read,0,s.next as u64*8);
        s.state=State::Ready;
    }
    pub fn submitted(&mut self) {
        for s in &mut self.slots { if s.state==State::Ready {
            s.mapped.store(0,Ordering::Release); let done=s.mapped.clone();
            s.read.map_async(MapMode::Read,0..s.next as u64*8,move |r|done.store(if r.is_ok(){1}else{2},Ordering::Release));
            s.state=State::Mapping;
        } }
    }
    pub fn collect(&mut self) {
        for i in 0..self.slots.len() {
            let s=&mut self.slots[i]; if s.state!=State::Mapping {continue;}
            let flag=s.mapped.load(Ordering::Acquire); if flag==0 {continue;}
            let report = if flag==1 {
                match s.read.get_mapped_range(0..s.next as u64*8) {
                Ok(data) => {
                let words:Vec<u64>=data.chunks_exact(8).map(|b|u64::from_le_bytes(b.try_into().unwrap())).collect();
                let elapsed=s.started.elapsed().as_nanos();
                let ambiguous=wrap_ambiguous(elapsed,self.valid_bits,self.period);
                let ns=|a:u32,b:u32| if ambiguous {None} else {ticks(words[a as usize],words[b as usize],self.valid_bits).map(|t|t as f64*self.period as f64)};
                let records:Vec<_>=s.records.iter().map(|r|serde_json::json!({"kind":r.kind,"label":r.label,"labelTruncated":r.label_truncated,"beginQuery":r.begin,"endQuery":r.end,"beginTick":words[r.begin as usize],"endTick":words[r.end as usize],"gpuNs":ns(r.begin,r.end)})).collect();
                serde_json::json!({"processId":std::process::id(),"frame":s.frame,"mapSuccess":true,"queryCount":s.next,
                    "frameSpanNs":ns(0,1),"frameBeginTick":words[0],"frameEndTick":words[1],"wrapAmbiguous":ambiguous,"hostCollectionDelayNs":elapsed.min(u64::MAX as u128) as u64,
                    "truncatedRegions":s.truncated,"droppedBusyFrames":self.dropped_busy,"mapErrors":self.map_errors,"records":records})
                },
                Err(e) => { self.map_errors+=1;serde_json::json!({"processId":std::process::id(),"frame":s.frame,"mapSuccess":false,"mapRangeError":format!("{e:?}"),"mapErrors":self.map_errors}) }
                }
            } else {self.map_errors+=1;serde_json::json!({"processId":std::process::id(),"frame":s.frame,"mapSuccess":false,"mapErrors":self.map_errors})};
            s.read.unmap(); s.state=State::Idle; s.records.clear();
            let line=format!("{report}\n");
            self.bytes=std::fs::metadata(self.dir.join("frames.jsonl")).map(|m|m.len()).unwrap_or(self.bytes);
            if self.bytes.saturating_add(line.len() as u64)>MAX_OUTPUT_BYTES {
                self.output_truncated=true;
                if let Ok(mut f)=std::fs::File::create(self.dir.join("truncated.json")) {let _=writeln!(f,"{{\"processId\":{},\"truncated\":true,\"bytes\":{}}}",std::process::id(),self.bytes);let _=f.sync_all();}
            } else if let Ok(mut f)=std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("frames.jsonl")) {
                if f.write_all(line.as_bytes()).is_ok(){self.bytes+=line.len() as u64;self.reports+=1;}else{self.io_errors+=1;}
            } else { self.io_errors+=1; }
            let pending=self.slots.iter().filter(|s|s.state==State::Mapping).count();
            let status=serde_json::json!({"processId":std::process::id(),"reports":self.reports,"bytes":self.bytes,"outputTruncated":self.output_truncated,"pendingMaps":pending,"droppedBusyFrames":self.dropped_busy,"mapErrors":self.map_errors,"ioErrors":self.io_errors});
            if let Ok(mut f)=std::fs::File::create(self.dir.join("status.json")) {let _=writeln!(f,"{status}");}
        }
    }
}
