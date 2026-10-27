//! Names of PCI devices from sysfs and the `pci.ids` database.

use std::path::Path;

/// Best effort: the sysfs `label` file, then the `pci.ids` database.
pub fn device_name(device_path: &Path) -> Option<String> {
    label(device_path).or_else(|| lookup(device_path))
}

fn label(device_path: &Path) -> Option<String> {
    let label = std::fs::read_to_string(device_path.join("label")).ok()?;
    let label = label.trim();
    (!label.is_empty()).then(|| label.to_string())
}

fn lookup(device_path: &Path) -> Option<String> {
    let read_id = |file: &str| {
        std::fs::read_to_string(device_path.join(file))
            .ok()
            .map(|s| s.trim().trim_start_matches("0x").to_lowercase())
    };
    let vendor = read_id("vendor")?;
    let device = read_id("device")?;
    let sub_vendor = read_id("subsystem_vendor").unwrap_or_default();
    let sub_device = read_id("subsystem_device").unwrap_or_default();
    ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"]
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .find_map(|content| search_pci_ids(&content, &vendor, &device, &sub_vendor, &sub_device))
}

/// The most specific name: the subsystem entry when it matches, else the device entry.
/// Format: vendor lines unindented, device lines after one tab, subsystem lines after two.
pub fn search_pci_ids(content: &str, vendor: &str, device: &str, sub_vendor: &str, sub_device: &str) -> Option<String> {
    let mut in_vendor = false;
    let mut device_name: Option<String> = None;
    for line in content.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                return device_name;
            }
            in_vendor = line.split_whitespace().next().is_some_and(|id| id.eq_ignore_ascii_case(vendor));
        } else if in_vendor && !line.starts_with("\t\t") {
            if device_name.is_some() {
                return device_name;
            }
            let trimmed = line.trim_start_matches('\t');
            if let Some((id, rest)) = trimmed.split_once(char::is_whitespace)
                && id.eq_ignore_ascii_case(device)
            {
                device_name = Some(rest.trim().to_string());
            }
        } else if device_name.is_some() && !sub_vendor.is_empty() && !sub_device.is_empty() {
            let mut parts = line.trim_start_matches('\t').split_whitespace();
            let (Some(sv), Some(sd)) = (parts.next(), parts.next()) else { continue };
            if sv.eq_ignore_ascii_case(sub_vendor) && sd.eq_ignore_ascii_case(sub_device) {
                return Some(parts.collect::<Vec<_>>().join(" "));
            }
        }
    }
    device_name
}

/// Kernel module bound to the device, e.g. `iwlwifi` or `amdgpu`.
pub fn driver_name(device_path: &Path) -> Option<String> {
    let driver = std::fs::read_link(device_path.join("driver")).ok()?;
    Some(driver.file_name()?.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: &str = "# comment\n1002  Advanced Micro Devices\n\t73bf  Navi 21 [Radeon RX 6800]\n\t\t1458 2329  Radeon RX 6800 GAMING OC\n\t73df  Navi 22\n8086  Intel Corporation\n\t46a6  Alder Lake-P GT2\n";

    #[test]
    fn prefers_the_subsystem_name() {
        assert_eq!(
            search_pci_ids(IDS, "1002", "73bf", "1458", "2329").as_deref(),
            Some("Radeon RX 6800 GAMING OC")
        );
    }

    #[test]
    fn falls_back_to_the_device_name() {
        assert_eq!(search_pci_ids(IDS, "1002", "73bf", "", "").as_deref(), Some("Navi 21 [Radeon RX 6800]"));
        assert_eq!(
            search_pci_ids(IDS, "1002", "73bf", "ffff", "0000").as_deref(),
            Some("Navi 21 [Radeon RX 6800]")
        );
        assert_eq!(search_pci_ids(IDS, "8086", "46a6", "", "").as_deref(), Some("Alder Lake-P GT2"));
    }

    #[test]
    fn unknown_ids_have_no_name() {
        assert_eq!(search_pci_ids(IDS, "1002", "ffff", "", ""), None);
        assert_eq!(search_pci_ids(IDS, "10de", "2204", "", ""), None);
    }
}
