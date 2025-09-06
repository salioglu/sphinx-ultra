use anyhow::{Context, Result};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use tokio::fs;

/// Inventory item representing a single object in the documentation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct InventoryItem {
    pub project_name: String,
    pub project_version: String,
    pub uri: String,
    pub display_name: String,
}

impl InventoryItem {
    pub fn new(
        project_name: String,
        project_version: String,
        uri: String,
        display_name: String,
    ) -> Self {
        Self {
            project_name,
            project_version,
            uri,
            display_name,
        }
    }
}

/// In-memory inventory data structure
#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub data: HashMap<String, HashMap<String, InventoryItem>>,
}

impl Inventory {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    /// Insert an item into the inventory
    pub fn insert(&mut self, obj_type: String, name: String, item: InventoryItem) {
        self.data
            .entry(obj_type)
            .or_insert_with(HashMap::new)
            .insert(name, item);
    }

    /// Get an item from the inventory
    pub fn get(&self, obj_type: &str, name: &str) -> Option<&InventoryItem> {
        self.data.get(obj_type)?.get(name)
    }

    /// Check if an item exists in the inventory
    pub fn contains(&self, obj_type: &str, name: &str) -> bool {
        self.data
            .get(obj_type)
            .map_or(false, |objects| objects.contains_key(name))
    }
}

/// Inventory file handler - mirrors Sphinx's InventoryFile class
pub struct InventoryFile;

impl InventoryFile {
    /// Load inventory from bytes (mirrors Sphinx's loads method)
    pub fn loads(content: &[u8], uri: &str) -> Result<Inventory> {
        let content_str = String::from_utf8_lossy(content);
        let mut lines = content_str.lines();

        // Parse header
        let format_line = lines.next().unwrap_or("").trim();

        if format_line == "# Sphinx inventory version 2" {
            Self::loads_v2(&mut lines, uri)
        } else if format_line == "# Sphinx inventory version 1" {
            Self::loads_v1(&mut lines, uri)
        } else if format_line.starts_with("# Sphinx inventory version ") {
            let version = &format_line[27..];
            anyhow::bail!("Unknown or unsupported inventory version: {}", version);
        } else {
            anyhow::bail!("Invalid inventory header: {}", format_line);
        }
    }

    /// Load inventory from version 1 format
    fn loads_v1(lines: &mut std::str::Lines, uri: &str) -> Result<Inventory> {
        let mut inv = Inventory::new();

        let project_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing project name"))?;
        let version_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing project version"))?;

        if !project_line.starts_with("# Project: ") || !version_line.starts_with("# Version: ") {
            anyhow::bail!("Invalid inventory header: missing project name or version");
        }

        let project_name = project_line[11..].trim();
        let version = version_line[11..].trim();

        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            if parts.len() != 3 {
                continue;
            }

            let name = parts[0];
            let item_type = parts[1];
            let location = parts[2];

            let full_location = if uri.is_empty() {
                location.to_string()
            } else {
                format!("{}/{}", uri.trim_end_matches('/'), location)
            };

            // Version 1 format conversion
            let (domain_type, anchor) = if item_type == "mod" {
                ("py:module".to_string(), format!("#module-{}", name))
            } else {
                (format!("py:{}", item_type), format!("#{}", name))
            };

            let item = InventoryItem::new(
                project_name.to_string(),
                version.to_string(),
                format!("{}{}", full_location, anchor),
                "-".to_string(),
            );

            inv.insert(domain_type, name.to_string(), item);
        }

        Ok(inv)
    }

    /// Load inventory from version 2 format
    fn loads_v2(lines: &mut std::str::Lines, uri: &str) -> Result<Inventory> {
        let mut inv = Inventory::new();

        let project_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing project name"))?;
        let version_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing project version"))?;
        let compression_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("Missing compression info"))?;

        if !project_line.starts_with("# Project: ") || !version_line.starts_with("# Version: ") {
            anyhow::bail!("Invalid inventory header: missing project name or version");
        }

        let project_name = project_line[11..].trim();
        let version = version_line[11..].trim();

        if !compression_line.contains("zlib") {
            anyhow::bail!(
                "Invalid inventory header (not compressed): {}",
                compression_line
            );
        }

        // Read the rest as compressed data
        let remaining_content: String = lines.collect::<Vec<_>>().join("\n");
        let compressed_data = {
            use base64::prelude::*;
            BASE64_STANDARD.decode(&remaining_content).or_else(|_| {
                // If base64 decode fails, try treating as raw bytes
                Ok::<Vec<u8>, base64::DecodeError>(remaining_content.as_bytes().to_vec())
            })?
        };

        // Decompress using zlib
        let decompressed = Self::decompress_zlib(&compressed_data)?;
        let decompressed_str = String::from_utf8(decompressed)?;

        // Parse inventory entries
        for line in decompressed_str.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Parse: name type priority location display_name
            let parts = Self::parse_inventory_line(line);
            if parts.len() != 5 {
                continue;
            }

            let name = parts[0];
            let obj_type = parts[1];
            let _priority = parts[2];
            let mut location = parts[3].to_string();
            let display_name = parts[4];

            // Skip invalid types
            if !obj_type.contains(':') {
                continue;
            }

            // Handle location anchors
            if location.ends_with('$') {
                location = location[..location.len() - 1].to_string() + name;
            }

            let full_location = if uri.is_empty() {
                location
            } else {
                format!("{}/{}", uri.trim_end_matches('/'), location)
            };

            let display_name = if display_name == "-" {
                name.to_string()
            } else {
                display_name.to_string()
            };

            let item = InventoryItem::new(
                project_name.to_string(),
                version.to_string(),
                full_location,
                display_name,
            );

            inv.insert(obj_type.to_string(), name.to_string(), item);
        }

        Ok(inv)
    }

    /// Parse a single inventory line, handling embedded spaces
    fn parse_inventory_line(line: &str) -> Vec<&str> {
        let regex = regex::Regex::new(r"(.+?)\s+(\S+)\s+(-?\d+)\s+?(\S*)\s+(.*)").unwrap();

        if let Some(captures) = regex.captures(line) {
            vec![
                captures.get(1).map_or("", |m| m.as_str()),
                captures.get(2).map_or("", |m| m.as_str()),
                captures.get(3).map_or("", |m| m.as_str()),
                captures.get(4).map_or("", |m| m.as_str()),
                captures.get(5).map_or("", |m| m.as_str()),
            ]
        } else {
            line.split_whitespace().collect()
        }
    }

    /// Decompress zlib data
    fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>> {
        use flate2::read::ZlibDecoder;
        use std::io::Read;

        let mut decoder = ZlibDecoder::new(data);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    }

    /// Dump inventory to file (mirrors Sphinx's dump method)
    pub async fn dump<P: AsRef<Path>>(
        filename: P,
        env: &crate::environment::BuildEnvironment,
        builder: &crate::html_builder::HTMLBuilder,
    ) -> Result<()> {
        info!("Dumping inventory to {}", filename.as_ref().display());

        let mut content = Vec::new();

        // Write header
        let project = &env.config.project;
        let version = env.config.version.as_deref().unwrap_or("");

        let header = format!(
            "# Sphinx inventory version 2\n# Project: {}\n# Version: {}\n# The remainder of this file is compressed using zlib.\n",
            Self::escape_string(project),
            Self::escape_string(version)
        );
        content.extend_from_slice(header.as_bytes());

        // Prepare inventory data
        let mut inventory_lines = Vec::new();

        // Collect all objects from all domains
        for (domain_name, domain) in &env.domains {
            let objects = domain.get_objects();
            for object in objects {
                let fullname = &object.name;
                let dispname = object.display_name.as_deref().unwrap_or(fullname);
                let obj_type = &object.object_type;
                let docname = &object.docname;
                let anchor = object.anchor.as_deref().unwrap_or("");
                let priority = object.priority;

                // Build URI
                let mut uri = builder.get_target_uri(docname);
                if !anchor.is_empty() {
                    if anchor.ends_with(fullname) {
                        // Optimize by using $ suffix
                        let prefix = &anchor[..anchor.len() - fullname.len()];
                        uri = format!("{}{}$", uri, prefix);
                    } else {
                        uri = format!("{}#{}", uri, anchor);
                    }
                }

                let final_dispname = if dispname == fullname { "-" } else { dispname };

                let entry = format!(
                    "{} {}:{} {} {} {}\n",
                    fullname, domain_name, obj_type, priority, uri, final_dispname
                );
                inventory_lines.push(entry);
            }
        }

        // Sort entries for consistency
        inventory_lines.sort();

        // Compress the inventory data
        let inventory_data = inventory_lines.join("");
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(inventory_data.as_bytes())?;
        let compressed_data = encoder.finish()?;

        content.extend_from_slice(&compressed_data);

        // Write to file
        fs::write(filename, content)
            .await
            .context("Failed to write inventory file")?;

        Ok(())
    }

    /// Escape string for inventory header
    fn escape_string(s: &str) -> String {
        regex::Regex::new(r"\s+")
            .unwrap()
            .replace_all(s, " ")
            .to_string()
    }

    /// Load inventory from file
    pub async fn load<P: AsRef<Path>>(filename: P, uri: &str) -> Result<Inventory> {
        let content = fs::read(filename.as_ref()).await.with_context(|| {
            format!(
                "Failed to read inventory file: {}",
                filename.as_ref().display()
            )
        })?;

        Self::loads(&content, uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inventory_item_creation() {
        let item = InventoryItem::new(
            "test_project".to_string(),
            "1.0".to_string(),
            "http://example.com/test.html".to_string(),
            "Test Item".to_string(),
        );

        assert_eq!(item.project_name, "test_project");
        assert_eq!(item.project_version, "1.0");
        assert_eq!(item.uri, "http://example.com/test.html");
        assert_eq!(item.display_name, "Test Item");
    }

    #[test]
    fn test_inventory_operations() {
        let mut inv = Inventory::new();

        let item = InventoryItem::new(
            "test".to_string(),
            "1.0".to_string(),
            "test.html".to_string(),
            "Test".to_string(),
        );

        inv.insert(
            "py:function".to_string(),
            "test_func".to_string(),
            item.clone(),
        );

        assert!(inv.contains("py:function", "test_func"));
        assert_eq!(inv.get("py:function", "test_func"), Some(&item));
        assert!(!inv.contains("py:function", "nonexistent"));
    }

    #[tokio::test]
    async fn test_parse_inventory_line() {
        let line = "test_function py:function 1 module.html#test_function Test Function";
        let parts = InventoryFile::parse_inventory_line(line);

        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0], "test_function");
        assert_eq!(parts[1], "py:function");
        assert_eq!(parts[2], "1");
        assert_eq!(parts[3], "module.html#test_function");
        assert_eq!(parts[4], "Test Function");
    }

    #[test]
    fn test_escape_string() {
        assert_eq!(
            InventoryFile::escape_string("test   multiple   spaces"),
            "test multiple spaces"
        );
        assert_eq!(InventoryFile::escape_string("test\ttab"), "test tab");
        assert_eq!(
            InventoryFile::escape_string("test\nnewline"),
            "test newline"
        );
    }
}
