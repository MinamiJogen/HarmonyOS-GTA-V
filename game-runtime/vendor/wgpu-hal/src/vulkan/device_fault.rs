//! Passive bounded VK_EXT_device_fault capture, only after a real device-loss result.
use ash::vk;
use alloc::{format, string::String, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};
use std::{fs::File, io::{self, Write}, path::{Path, PathBuf}};

pub(super) const MAX_ADDRESSES: u32 = 1024;
pub(super) const MAX_VENDOR_INFOS: u32 = 1024;
pub(super) const MAX_VENDOR_BINARY: u64 = 8 << 20;

pub(super) struct Diagnostics {
    query: Option<vk::PFN_vkGetDeviceFaultInfoEXT>,
    directory: Option<PathBuf>,
    binary_enabled: bool,
    claimed: AtomicBool,
}

fn text(bytes: &[core::ffi::c_char]) -> String {
    let bytes: Vec<u8> = bytes.iter().take_while(|&&b| b != 0).map(|&b| b as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write_synced(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(content)?;
    file.sync_all()
}

fn initialized<T: Default + Clone>(count: usize) -> io::Result<Vec<T>> {
    let mut values = Vec::new();
    values.try_reserve_exact(count).map_err(|e| io::Error::other(format!("bounded allocation failed: {e}")))?;
    values.resize(count, T::default());
    Ok(values)
}

impl Diagnostics {
    pub(super) fn new(instance: &ash::Instance, device: &ash::Device, advertised: bool,
        queried: Option<(bool, bool)>, extension_enabled: bool) -> Self {
        let directory = std::env::var_os("MUGUET_DEVICE_FAULT_DIR").map(PathBuf::from);
        let feature_enabled = extension_enabled && queried.is_some_and(|f| f.0);
        let binary_enabled = feature_enabled && queried.is_some_and(|f| f.1);
        // Avoid ash's panic stub when an advertised extension lacks an entrypoint.
        let query = if feature_enabled {
            unsafe { instance.get_device_proc_addr(device.handle(), c"vkGetDeviceFaultInfoEXT".as_ptr()) }
                .map(|p| unsafe { core::mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkGetDeviceFaultInfoEXT>(p) })
        } else { None };
        if let Some(path) = directory.as_deref() {
            let record = format!("extension=VK_EXT_device_fault\nprocessId={}\nadvertised={advertised}\nfeatures2Queried={}\nphysicalDeviceFault={}\nphysicalDeviceFaultVendorBinary={}\nextensionEnabled={extension_enabled}\nlogicalDeviceFaultEnabled={feature_enabled}\nlogicalDeviceFaultVendorBinaryEnabled={binary_enabled}\nentrypointLoaded={}\nmaxAddresses={MAX_ADDRESSES}\nmaxVendorInfos={MAX_VENDOR_INFOS}\nmaxVendorBinaryBytes={MAX_VENDOR_BINARY}\nmaxQueryCalls=2\n", std::process::id(), queried.is_some(), queried.map_or(false, |f| f.0), queried.map_or(false, |f| f.1), query.is_some());
            if std::fs::create_dir_all(path).and_then(|()| {
                for name in ["vendor-binary.bin", "capture-error.txt"] {
                    match std::fs::remove_file(path.join(name)) {
                        Ok(()) => {}, Err(e) if e.kind() == io::ErrorKind::NotFound => {}, Err(e) => return Err(e),
                    }
                }
                write_synced(&path.join("device-fault.txt"), b"status=armed\nqueryCalls=0\nno device loss observed\n")?;
                write_synced(&path.join("capabilities.txt"), record.as_bytes())
            }).is_err() {
                log::error!("Cannot persist Vulkan device-fault capabilities at {}", path.display());
            }
        }
        Self { query, directory, binary_enabled, claimed: AtomicBool::new(false) }
    }

    pub(super) fn capture(&self, device: vk::Device, reason: &str) {
        let Some(path) = self.directory.as_deref() else { return; };
        if self.claimed.swap(true, Ordering::AcqRel) { return; }
        let Some(query) = self.query else {
            let _ = write_synced(&path.join("device-fault.txt"), format!("status=unavailable\nreason={reason:?}\nfeature or entrypoint not enabled; no fault query made\n").as_bytes());
            return;
        };
        // Caller has observed ERROR_DEVICE_LOST, satisfying 07336. One caller
        // per device; retain the original error and abort/return behavior.
        let result = collect(path, reason, self.binary_enabled, |counts, info| unsafe { query(device, counts, info) });
        if let Err(error) = result {
            let _ = write_synced(&path.join("capture-error.txt"), format!("reason={reason:?}\nerror={error}\n").as_bytes());
        }
    }
}

/// Two calls maximum: count query followed by one bounded fill. INCOMPLETE is
/// recorded as partial evidence, never retried indefinitely or called a full dump.
pub(super) fn collect<F>(directory: &Path, reason: &str, binary_enabled: bool, mut query: F) -> io::Result<()>
where F: FnMut(&mut vk::DeviceFaultCountsEXT<'_>, *mut vk::DeviceFaultInfoEXT<'_>) -> vk::Result {
    let mut file = File::create(directory.join("device-fault.txt"))?;
    writeln!(file, "phase=before-counts\nreason={reason:?}\noriginalError=ERROR_DEVICE_LOST\nmaxQueryCalls=2")?;
    file.sync_all()?;
    let mut counts = vk::DeviceFaultCountsEXT::default();
    let result = query(&mut counts, core::ptr::null_mut());
    writeln!(file, "phase=after-counts\ncountsResult={result:?}\nreportedAddresses={}\nreportedVendorInfos={}\nreportedVendorBinaryBytes={}", counts.address_info_count, counts.vendor_info_count, counts.vendor_binary_size)?;
    file.sync_all()?;
    if result != vk::Result::SUCCESS && result != vk::Result::INCOMPLETE { return Ok(()); }
    let address_capacity = counts.address_info_count.min(MAX_ADDRESSES);
    let vendor_capacity = counts.vendor_info_count.min(MAX_VENDOR_INFOS);
    let binary_capacity = if binary_enabled { counts.vendor_binary_size.min(MAX_VENDOR_BINARY) } else { 0 };
    let clipped = address_capacity < counts.address_info_count || vendor_capacity < counts.vendor_info_count || binary_capacity < counts.vendor_binary_size;
    let mut addresses = initialized::<vk::DeviceFaultAddressInfoEXT>(address_capacity as usize)?;
    let mut vendors = initialized::<vk::DeviceFaultVendorInfoEXT>(vendor_capacity as usize)?;
    let mut binary = initialized::<u8>(binary_capacity as usize)?;
    let mut info = vk::DeviceFaultInfoEXT::default();
    info.p_address_infos = if addresses.is_empty() { core::ptr::null_mut() } else { addresses.as_mut_ptr() };
    info.p_vendor_infos = if vendors.is_empty() { core::ptr::null_mut() } else { vendors.as_mut_ptr() };
    info.p_vendor_binary_data = if binary.is_empty() { core::ptr::null_mut() } else { binary.as_mut_ptr().cast() };
    counts = vk::DeviceFaultCountsEXT::default().address_info_count(address_capacity)
        .vendor_info_count(vendor_capacity).vendor_binary_size(binary_capacity);
    writeln!(file, "phase=before-info\naddressCapacity={address_capacity}\nvendorCapacity={vendor_capacity}\nvendorBinaryCapacity={binary_capacity}\ncapacityClipped={clipped}")?;
    file.sync_all()?;
    let result = query(&mut counts, &mut info);
    writeln!(file, "phase=after-info\ninfoResult={result:?}\nwrittenAddresses={}\nwrittenVendorInfos={}\nwrittenVendorBinaryBytes={}", counts.address_info_count, counts.vendor_info_count, counts.vendor_binary_size)?;
    if result != vk::Result::SUCCESS && result != vk::Result::INCOMPLETE { file.sync_all()?; return Ok(()); }
    let invalid = counts.address_info_count > address_capacity || counts.vendor_info_count > vendor_capacity || counts.vendor_binary_size > binary_capacity;
    writeln!(file, "driverReturnedOutOfCapacityCount={invalid}\ncomplete={}\ndescription={:?}", !clipped && !invalid && result == vk::Result::SUCCESS, text(&info.description))?;
    for (index, address) in addresses.iter().take(counts.address_info_count.min(address_capacity) as usize).enumerate() {
        writeln!(file, "address[{index}] type={:?} reportedAddress={:#x} addressPrecision={:#x}", address.address_type, address.reported_address, address.address_precision)?;
    }
    for (index, vendor) in vendors.iter().take(counts.vendor_info_count.min(vendor_capacity) as usize).enumerate() {
        writeln!(file, "vendor[{index}] description={:?} faultCode={:#x} faultData={:#x}", text(&vendor.description), vendor.vendor_fault_code, vendor.vendor_fault_data)?;
    }
    file.sync_all()?;
    let written = counts.vendor_binary_size.min(binary_capacity) as usize;
    if written != 0 { write_synced(&directory.join("vendor-binary.bin"), &binary[..written])?; }
    writeln!(file, "phase=complete\nvendorBinaryPersistedBytes={written}")?;
    file.sync_all()
}

impl super::DeviceShared {
    pub(super) fn capture_device_fault_if_lost(&self, error: vk::Result, reason: &str) {
        if error == vk::Result::ERROR_DEVICE_LOST { self.device_fault.capture(self.raw.handle(), reason); }
    }
}
