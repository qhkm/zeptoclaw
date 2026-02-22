//! Screen perception — uiautomator XML parsing, scoring, and compact output.
//!
//! Parses the XML output of `uiautomator dump` into scored UI elements,
//! deduplicates by spatial proximity, and produces token-efficient JSON
//! for the LLM.

use std::collections::HashMap;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::{Result, ZeptoError};

use super::types::UIElement;

/// Default maximum number of elements to return.
const DEFAULT_TOP_N: usize = 30;

/// Pixel tolerance for deduplication bucket.
const DEDUP_TOLERANCE: i32 = 5;

/// Parse uiautomator XML dump into UI elements.
///
/// Performs a depth-first traversal of the XML, extracting bounds, text,
/// content-desc, resource-id, class, and state flags from each `<node>`.
pub fn parse_ui_dump(xml: &str) -> Result<Vec<UIElement>> {
    let mut reader = Reader::from_str(xml);
    let mut elements = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == b"node" {
                    if let Some(elem) = parse_node_attributes(e) {
                        elements.push(elem);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(ZeptoError::Tool(format!(
                    "XML parse error at position {}: {}",
                    reader.buffer_position(),
                    e
                )));
            }
            _ => {}
        }
    }

    Ok(elements)
}

/// Parse attributes from a single `<node>` element.
fn parse_node_attributes(e: &quick_xml::events::BytesStart<'_>) -> Option<UIElement> {
    let mut text = String::new();
    let mut content_desc = String::new();
    let mut resource_id = String::new();
    let mut class = String::new();
    let mut bounds_str = String::new();
    let mut clickable = false;
    let mut enabled = true;
    let mut checked = false;
    let mut focused = false;
    let mut editable = false;
    let mut scrollable = false;

    for attr in e.attributes().flatten() {
        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
        let val = String::from_utf8_lossy(&attr.value).to_string();

        match key {
            "text" => text = val,
            "content-desc" => content_desc = val,
            "resource-id" => resource_id = val,
            "class" => class = val,
            "bounds" => bounds_str = val,
            "clickable" => clickable = val == "true",
            "enabled" => enabled = val == "true",
            "checked" => checked = val == "true",
            "focused" => focused = val == "true",
            "scrollable" => scrollable = val == "true",
            _ => {}
        }
    }

    // Parse bounds "[x1,y1][x2,y2]" -> center
    let (cx, cy) = parse_bounds(&bounds_str)?;

    // Determine if editable (EditText class)
    if class.contains("EditText") {
        editable = true;
    }

    // Build display text: prefer visible text, fall back to content-desc
    let display_text = if !text.is_empty() {
        text.clone()
    } else if !content_desc.is_empty() {
        content_desc.clone()
    } else {
        String::new()
    };

    // Skip invisible/empty elements with no ID
    if display_text.is_empty() && resource_id.is_empty() && !scrollable {
        return None;
    }

    // Determine suggested action
    let action = if editable {
        "type"
    } else if scrollable {
        "scroll"
    } else {
        "tap"
    };

    // Compute relevance score
    let score = compute_score(
        enabled,
        editable,
        focused,
        clickable,
        scrollable,
        &display_text,
    );

    // Short class name
    let short_class = class.rsplit('.').next().map(|s| s.to_string());

    // Hint for editable fields (use content-desc if text differs)
    let hint = if editable && !content_desc.is_empty() && content_desc != text {
        Some(content_desc)
    } else {
        None
    };

    // Short resource ID (strip package prefix)
    let short_id = if resource_id.is_empty() {
        None
    } else {
        Some(
            resource_id
                .rsplit('/')
                .next()
                .unwrap_or(&resource_id)
                .to_string(),
        )
    };

    Some(UIElement {
        text: display_text,
        center: [cx, cy],
        action: action.to_string(),
        class: short_class,
        id: short_id,
        hint,
        enabled,
        checked,
        focused,
        editable,
        scrollable,
        score,
    })
}

/// Parse bounds string `"[x1,y1][x2,y2]"` into center `(cx, cy)`.
fn parse_bounds(bounds: &str) -> Option<(i32, i32)> {
    // "[0,0][1080,2400]"
    let nums: Vec<i32> = bounds
        .replace('[', "")
        .replace(']', ",")
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    if nums.len() == 4 {
        let cx = (nums[0] + nums[2]) / 2;
        let cy = (nums[1] + nums[3]) / 2;
        Some((cx, cy))
    } else {
        None
    }
}

/// Compute relevance score for an element.
fn compute_score(
    enabled: bool,
    editable: bool,
    focused: bool,
    clickable: bool,
    scrollable: bool,
    text: &str,
) -> i32 {
    let mut score = 0;
    if enabled {
        score += 10;
    }
    if editable {
        score += 8;
    }
    if focused {
        score += 6;
    }
    if clickable {
        score += 5;
    }
    if scrollable {
        score += 3;
    }
    if !text.is_empty() {
        score += 3;
    }
    score
}

/// Deduplicate elements by spatial proximity.
///
/// Elements with centers within `DEDUP_TOLERANCE` pixels are bucketed
/// together; the highest-scored element in each bucket wins.
pub fn dedup_elements(elements: Vec<UIElement>) -> Vec<UIElement> {
    let mut buckets: HashMap<(i32, i32), UIElement> = HashMap::new();

    for elem in elements {
        let bx = elem.center[0] / DEDUP_TOLERANCE;
        let by = elem.center[1] / DEDUP_TOLERANCE;
        let key = (bx, by);

        let entry = buckets.entry(key).or_insert(elem.clone());
        if elem.score > entry.score {
            *entry = elem;
        }
    }

    buckets.into_values().collect()
}

/// Filter, score, dedup, and select top-N elements.
pub fn process_elements(mut elements: Vec<UIElement>, top_n: Option<usize>) -> Vec<UIElement> {
    let limit = top_n.unwrap_or(DEFAULT_TOP_N);

    // Dedup
    elements = dedup_elements(elements);

    // Sort by score descending, then by y, then by x for stability
    elements.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.center[1].cmp(&b.center[1]))
            .then_with(|| a.center[0].cmp(&b.center[0]))
    });

    // Take top-N
    elements.truncate(limit);

    // Re-sort by position (top to bottom, left to right) for natural reading order
    elements.sort_by(|a, b| {
        a.center[1]
            .cmp(&b.center[1])
            .then_with(|| a.center[0].cmp(&b.center[0]))
    });

    elements
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<hierarchy rotation="0">
  <node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.example" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[0,0][1080,2400]">
    <node index="0" text="Sign In" resource-id="com.example:id/btn_signin" class="android.widget.Button" package="com.example" content-desc="" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[200,800][880,920]" />
    <node index="1" text="" resource-id="com.example:id/input_email" class="android.widget.EditText" package="com.example" content-desc="Email address" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="true" scrollable="false" long-clickable="true" password="false" selected="false" bounds="[100,500][980,600]" />
    <node index="2" text="Remember me" resource-id="com.example:id/cb_remember" class="android.widget.CheckBox" package="com.example" content-desc="" checkable="true" checked="true" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[100,650][400,700]" />
    <node index="3" text="" resource-id="" class="android.view.View" package="com.example" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[0,950][1080,960]" />
    <node index="4" text="Forgot password?" resource-id="" class="android.widget.TextView" package="com.example" content-desc="" checkable="false" checked="false" clickable="true" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[300,970][780,1010]" />
    <node index="5" text="" resource-id="com.example:id/scroll_list" class="android.widget.ScrollView" package="com.example" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="true" long-clickable="false" password="false" selected="false" bounds="[0,1100][1080,2200]" />
    <node index="6" text="Disabled button" resource-id="" class="android.widget.Button" package="com.example" content-desc="" checkable="false" checked="false" clickable="true" enabled="false" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[200,2250][880,2350]" />
  </node>
</hierarchy>"#;

    #[test]
    fn test_parse_bounds() {
        assert_eq!(parse_bounds("[0,0][1080,2400]"), Some((540, 1200)));
        assert_eq!(parse_bounds("[200,800][880,920]"), Some((540, 860)));
        assert_eq!(parse_bounds("invalid"), None);
        assert_eq!(parse_bounds(""), None);
    }

    #[test]
    fn test_compute_score() {
        // Enabled + clickable + text
        assert_eq!(compute_score(true, false, false, true, false, "OK"), 18);
        // Editable + enabled + focused + text
        assert_eq!(compute_score(true, true, true, false, false, "hi"), 27);
        // Disabled, no text
        assert_eq!(compute_score(false, false, false, false, false, ""), 0);
    }

    #[test]
    fn test_parse_ui_dump_sample() {
        let elements = parse_ui_dump(SAMPLE_XML).unwrap();
        // Should skip the invisible empty view (index=3) and root frame
        assert!(elements.len() >= 5);

        // Check "Sign In" button
        let signin = elements.iter().find(|e| e.text == "Sign In").unwrap();
        assert_eq!(signin.center, [540, 860]);
        assert_eq!(signin.action, "tap");
        assert!(signin.enabled);

        // Check email EditText
        let email = elements.iter().find(|e| e.editable).unwrap();
        assert_eq!(email.action, "type");
        assert!(email.focused);
        assert_eq!(email.hint.as_deref(), Some("Email address"));
        assert_eq!(email.id.as_deref(), Some("input_email"));

        // Check checkbox
        let remember = elements.iter().find(|e| e.text == "Remember me").unwrap();
        assert!(remember.checked);

        // Check scroll view
        let scroll = elements.iter().find(|e| e.scrollable).unwrap();
        assert_eq!(scroll.action, "scroll");
        assert_eq!(scroll.id.as_deref(), Some("scroll_list"));

        // Check disabled button
        let disabled = elements
            .iter()
            .find(|e| e.text == "Disabled button")
            .unwrap();
        assert!(!disabled.enabled);
    }

    #[test]
    fn test_parse_ui_dump_invalid_xml() {
        let result = parse_ui_dump("<not_closed");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_ui_dump_empty() {
        let elements = parse_ui_dump("<?xml version=\"1.0\"?><hierarchy/>").unwrap();
        assert!(elements.is_empty());
    }

    #[test]
    fn test_dedup_elements() {
        let elems = vec![
            UIElement {
                text: "A".into(),
                center: [100, 200],
                action: "tap".into(),
                class: None,
                id: None,
                hint: None,
                enabled: true,
                checked: false,
                focused: false,
                editable: false,
                scrollable: false,
                score: 10,
            },
            UIElement {
                text: "B".into(),
                center: [102, 201], // within 5px bucket
                action: "tap".into(),
                class: None,
                id: None,
                hint: None,
                enabled: true,
                checked: false,
                focused: false,
                editable: false,
                scrollable: false,
                score: 15,
            },
            UIElement {
                text: "C".into(),
                center: [500, 500], // different bucket
                action: "tap".into(),
                class: None,
                id: None,
                hint: None,
                enabled: true,
                checked: false,
                focused: false,
                editable: false,
                scrollable: false,
                score: 5,
            },
        ];

        let deduped = dedup_elements(elems);
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn test_process_elements_top_n() {
        let elements = parse_ui_dump(SAMPLE_XML).unwrap();
        let processed = process_elements(elements, Some(3));
        assert_eq!(processed.len(), 3);
    }

    #[test]
    fn test_process_elements_sorted_by_position() {
        let elements = parse_ui_dump(SAMPLE_XML).unwrap();
        let processed = process_elements(elements, Some(10));
        // Elements should be sorted by y-coordinate (top to bottom)
        for window in processed.windows(2) {
            assert!(window[0].center[1] <= window[1].center[1]);
        }
    }

    #[test]
    fn test_parse_bounds_edge_cases() {
        // Single pixel element
        assert_eq!(parse_bounds("[10,10][10,10]"), Some((10, 10)));
        // Large coordinates
        assert_eq!(parse_bounds("[0,0][2160,3840]"), Some((1080, 1920)));
    }

    #[test]
    fn test_short_class_name() {
        let xml = r#"<?xml version="1.0"?><hierarchy><node text="X" class="android.widget.Button" bounds="[0,0][100,100]" clickable="true" enabled="true" checked="false" focused="false" scrollable="false" resource-id="" content-desc="" /></hierarchy>"#;
        let elements = parse_ui_dump(xml).unwrap();
        assert_eq!(elements[0].class.as_deref(), Some("Button"));
    }

    #[test]
    fn test_short_resource_id() {
        let xml = r#"<?xml version="1.0"?><hierarchy><node text="X" class="android.widget.Button" bounds="[0,0][100,100]" clickable="true" enabled="true" checked="false" focused="false" scrollable="false" resource-id="com.app:id/my_button" content-desc="" /></hierarchy>"#;
        let elements = parse_ui_dump(xml).unwrap();
        assert_eq!(elements[0].id.as_deref(), Some("my_button"));
    }

    #[test]
    fn test_content_desc_as_text() {
        let xml = r#"<?xml version="1.0"?><hierarchy><node text="" class="android.widget.ImageButton" bounds="[0,0][100,100]" clickable="true" enabled="true" checked="false" focused="false" scrollable="false" resource-id="" content-desc="Navigate up" /></hierarchy>"#;
        let elements = parse_ui_dump(xml).unwrap();
        assert_eq!(elements[0].text, "Navigate up");
    }

    #[test]
    fn test_skip_invisible_empty() {
        let xml = r#"<?xml version="1.0"?><hierarchy><node text="" class="android.view.View" bounds="[0,0][100,100]" clickable="false" enabled="true" checked="false" focused="false" scrollable="false" resource-id="" content-desc="" /></hierarchy>"#;
        let elements = parse_ui_dump(xml).unwrap();
        assert!(elements.is_empty());
    }

    #[test]
    fn test_editable_via_class() {
        let xml = r#"<?xml version="1.0"?><hierarchy><node text="hello" class="android.widget.EditText" bounds="[0,0][100,100]" clickable="true" enabled="true" checked="false" focused="false" scrollable="false" resource-id="com.app:id/input" content-desc="" /></hierarchy>"#;
        let elements = parse_ui_dump(xml).unwrap();
        assert!(elements[0].editable);
        assert_eq!(elements[0].action, "type");
    }
}
