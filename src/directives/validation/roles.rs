//! Built-in role validators for common Sphinx roles

use super::{ParsedRole, RoleValidationResult, RoleValidator};

/// Validator for doc role
#[derive(Default)]
pub struct DocRoleValidator;

impl DocRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for DocRoleValidator {
    fn name(&self) -> &str {
        "doc"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error("Doc role requires a document target".to_string());
        }

        // Check for valid document path format
        if role.target.contains("..") {
            return RoleValidationResult::Warning(
                "Document path contains parent directory references".to_string(),
            );
        }

        // Check for common document extensions
        if role.target.ends_with(".rst") || role.target.ends_with(".md") {
            return RoleValidationResult::Warning(
                "Document reference should not include file extension".to_string(),
            );
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        true
    }
}

/// Validator for ref role
#[derive(Default)]
pub struct RefRoleValidator;

impl RefRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for RefRoleValidator {
    fn name(&self) -> &str {
        "ref"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error("Ref role requires a reference target".to_string());
        }

        // Check for spaces first (this is an error)
        if role.target.contains(' ') {
            return RoleValidationResult::Error(
                "Reference targets cannot contain spaces".to_string(),
            );
        }

        // Check for valid reference format (lowercase, hyphens/underscores)
        if !role
            .target
            .chars()
            .all(|c| c.is_lowercase() || c.is_numeric() || c == '-' || c == '_')
        {
            return RoleValidationResult::Warning(
                "Reference targets should use lowercase letters, numbers, hyphens, and underscores"
                    .to_string(),
            );
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        true
    }
}

/// Validator for download role
#[derive(Default)]
pub struct DownloadRoleValidator;

impl DownloadRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for DownloadRoleValidator {
    fn name(&self) -> &str {
        "download"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error("Download role requires a file path".to_string());
        }

        // Check for potentially downloadable file types
        let downloadable_extensions = [
            "pdf", "zip", "tar", "gz", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "csv",
            "json", "xml", "sql", "py", "rs", "js", "cpp", "c", "h", "java", "go", "rb", "php",
        ];

        if let Some(extension) = role.target.split('.').next_back() {
            if !downloadable_extensions.contains(&extension.to_lowercase().as_str()) {
                return RoleValidationResult::Warning(format!(
                    "Unusual file type for download: {}",
                    extension
                ));
            }
        } else {
            return RoleValidationResult::Warning(
                "Download target has no file extension".to_string(),
            );
        }

        // Check for absolute paths or URLs
        if role.target.starts_with("http://") || role.target.starts_with("https://") {
            return RoleValidationResult::Warning(
                "Download role should reference local files, not URLs".to_string(),
            );
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        true
    }
}

/// Validator for math role
#[derive(Default)]
pub struct MathRoleValidator;

impl MathRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for MathRoleValidator {
    fn name(&self) -> &str {
        "math"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error(
                "Math role requires LaTeX math expression".to_string(),
            );
        }

        // Basic LaTeX syntax check
        let open_braces = role.target.matches('{').count();
        let close_braces = role.target.matches('}').count();

        if open_braces != close_braces {
            return RoleValidationResult::Warning(
                "Unmatched braces in math expression".to_string(),
            );
        }

        // Check for common LaTeX commands
        if role.target.contains('\\')
            && !role.target.contains("\\frac")
            && !role.target.contains("\\sqrt")
        {
            // This is a very basic check; real validation would be much more comprehensive
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for abbreviation role
#[derive(Default)]
pub struct AbbreviationRoleValidator;

impl AbbreviationRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for AbbreviationRoleValidator {
    fn name(&self) -> &str {
        "abbr"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error("Abbreviation role requires text".to_string());
        }

        // Check for typical abbreviation format
        if !role.target.chars().any(|c| c.is_uppercase()) {
            return RoleValidationResult::Warning(
                "Abbreviations typically contain uppercase letters".to_string(),
            );
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        true
    }
}

/// Validator for command role
#[derive(Default)]
pub struct CommandRoleValidator;

impl CommandRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for CommandRoleValidator {
    fn name(&self) -> &str {
        "command"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error("Command role requires a command name".to_string());
        }

        // Check for shell injection characters
        let dangerous_chars = ['&', '|', ';', '`', '$', '(', ')', '<', '>'];
        if role.target.chars().any(|c| dangerous_chars.contains(&c)) {
            return RoleValidationResult::Warning(
                "Command contains potentially dangerous characters".to_string(),
            );
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for file role
#[derive(Default)]
pub struct FileRoleValidator;

impl FileRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for FileRoleValidator {
    fn name(&self) -> &str {
        "file"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error("File role requires a file path".to_string());
        }

        // Check for valid file path characters
        let invalid_chars = ['<', '>', ':', '"', '|', '?', '*'];
        if role.target.chars().any(|c| invalid_chars.contains(&c)) {
            return RoleValidationResult::Error(
                "File path contains invalid characters".to_string(),
            );
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for kbd role
#[derive(Default)]
pub struct KbdRoleValidator;

impl KbdRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for KbdRoleValidator {
    fn name(&self) -> &str {
        "kbd"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error("Kbd role requires key combination".to_string());
        }

        // Check for common key patterns
        let common_keys = [
            "Ctrl",
            "Alt",
            "Shift",
            "Enter",
            "Escape",
            "Tab",
            "Space",
            "F1",
            "F2",
            "F3",
            "F4",
            "F5",
            "F6",
            "F7",
            "F8",
            "F9",
            "F10",
            "F11",
            "F12",
            "Home",
            "End",
            "Page Up",
            "Page Down",
            "Delete",
            "Insert",
        ];

        // Split by common separators
        let keys: Vec<&str> = role.target.split(['+', '-']).collect();

        for key in &keys {
            let key = key.trim();
            if !key.is_empty() && !common_keys.contains(&key) && key.len() > 1 {
                return RoleValidationResult::Warning(format!("Unusual key name: {}", key));
            }
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for menuselection role
#[derive(Default)]
pub struct MenuSelectionRoleValidator;

impl MenuSelectionRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for MenuSelectionRoleValidator {
    fn name(&self) -> &str {
        "menuselection"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error(
                "Menu selection role requires menu path".to_string(),
            );
        }

        // Check for typical menu separator
        if !role.target.contains("-->") && !role.target.contains(" > ") {
            return RoleValidationResult::Warning(
                "Menu selection should use '-->' or ' > ' as separator".to_string(),
            );
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

/// Validator for guilabel role
#[derive(Default)]
pub struct GuiLabelRoleValidator;

impl GuiLabelRoleValidator {
    pub fn new() -> Self {
        Self
    }
}

impl RoleValidator for GuiLabelRoleValidator {
    fn name(&self) -> &str {
        "guilabel"
    }

    fn validate(&self, role: &ParsedRole) -> RoleValidationResult {
        if role.target.is_empty() {
            return RoleValidationResult::Error("GUI label role requires label text".to_string());
        }

        // Check for ampersand (access key indicator)
        if role.target.contains('&') && !role.target.contains("&amp;") {
            return RoleValidationResult::Warning(
                "Use &amp; for literal ampersand in GUI labels".to_string(),
            );
        }

        RoleValidationResult::Valid
    }

    fn requires_target(&self) -> bool {
        true
    }

    fn allows_display_text(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directives::validation::SourceLocation;

    fn create_test_role(name: &str, target: &str, display_text: Option<String>) -> ParsedRole {
        ParsedRole {
            name: name.to_string(),
            target: target.to_string(),
            display_text,
            location: SourceLocation {
                file: "test.rst".to_string(),
                line: 1,
                column: 1,
            },
        }
    }

    #[test]
    fn test_doc_role_validator() {
        let validator = DocRoleValidator::new();

        // Valid doc role
        let role = create_test_role("doc", "installation", None);
        assert_eq!(validator.validate(&role), RoleValidationResult::Valid);

        // Empty target
        let role = create_test_role("doc", "", None);
        assert!(matches!(
            validator.validate(&role),
            RoleValidationResult::Error(_)
        ));

        // With extension
        let role = create_test_role("doc", "installation.rst", None);
        assert!(matches!(
            validator.validate(&role),
            RoleValidationResult::Warning(_)
        ));
    }

    #[test]
    fn test_ref_role_validator() {
        let validator = RefRoleValidator::new();

        // Valid ref role
        let role = create_test_role("ref", "advanced-usage", None);
        assert_eq!(validator.validate(&role), RoleValidationResult::Valid);

        // With spaces
        let role = create_test_role("ref", "advanced usage", None);
        assert!(matches!(
            validator.validate(&role),
            RoleValidationResult::Error(_)
        ));

        // With uppercase
        let role = create_test_role("ref", "Advanced-Usage", None);
        assert!(matches!(
            validator.validate(&role),
            RoleValidationResult::Warning(_)
        ));
    }

    #[test]
    fn test_download_role_validator() {
        let validator = DownloadRoleValidator::new();

        // Valid download role
        let role = create_test_role("download", "example.pdf", None);
        assert_eq!(validator.validate(&role), RoleValidationResult::Valid);

        // No extension
        let role = create_test_role("download", "example", None);
        assert!(matches!(
            validator.validate(&role),
            RoleValidationResult::Warning(_)
        ));

        // URL
        let role = create_test_role("download", "https://example.com/file.pdf", None);
        assert!(matches!(
            validator.validate(&role),
            RoleValidationResult::Warning(_)
        ));
    }

    #[test]
    fn test_math_role_validator() {
        let validator = MathRoleValidator::new();

        // Valid math role
        let role = create_test_role("math", "x = y + z", None);
        assert_eq!(validator.validate(&role), RoleValidationResult::Valid);

        // Empty target
        let role = create_test_role("math", "", None);
        assert!(matches!(
            validator.validate(&role),
            RoleValidationResult::Error(_)
        ));

        // Unmatched braces
        let role = create_test_role("math", "x = \\frac{a}{b", None);
        assert!(matches!(
            validator.validate(&role),
            RoleValidationResult::Warning(_)
        ));
    }

    #[test]
    fn test_kbd_role_validator() {
        let validator = KbdRoleValidator::new();

        // Valid kbd role
        let role = create_test_role("kbd", "Ctrl+C", None);
        assert_eq!(validator.validate(&role), RoleValidationResult::Valid);

        // Empty target
        let role = create_test_role("kbd", "", None);
        assert!(matches!(
            validator.validate(&role),
            RoleValidationResult::Error(_)
        ));
    }
}
