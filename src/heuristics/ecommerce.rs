//! E-commerce heuristic: detect cart, checkout, and purchase flow elements.
//!
//! Matches goal keywords (cart, buy, checkout, pay) against button labels
//! to click add-to-cart and checkout buttons.

use crate::pilot::Action;
use crate::semantic::{ElementKind, SemanticView};

/// Button labels for add-to-cart actions.
const CART_BUTTON_PATTERNS: &[&str] = &[
    "add to cart",
    "add to bag",
    "buy now",
    "purchase",
    "order now",
    "add to basket",
];

/// Button labels for checkout / payment actions.
const CHECKOUT_PATTERNS: &[&str] = &[
    "checkout",
    "proceed to checkout",
    "continue to payment",
    "place order",
    "complete purchase",
    "pay now",
    "confirm order",
];

/// Try to click an add-to-cart or checkout button based on goal keywords.
///
/// Only activates when the goal contains e-commerce keywords like
/// "cart", "buy", "checkout", "pay", or "purchase".
pub fn try_ecommerce_action(
    view: &SemanticView,
    goal: &str,
    acted_on: &[u32],
) -> Option<super::HeuristicResult> {
    let goal_lower = goal.to_lowercase();

    let patterns: &[&str] = if goal_lower.contains("checkout") || goal_lower.contains("pay") {
        CHECKOUT_PATTERNS
    } else if goal_lower.contains("cart")
        || goal_lower.contains("buy")
        || goal_lower.contains("add")
        || goal_lower.contains("purchase")
    {
        CART_BUTTON_PATTERNS
    } else {
        return None;
    };

    for el in &view.elements {
        if acted_on.contains(&el.id) || el.disabled {
            continue;
        }
        if !matches!(el.kind, ElementKind::Button | ElementKind::Link) {
            continue;
        }

        let label_lower = el.label.to_lowercase();
        if patterns.iter().any(|p| label_lower.contains(p)) {
            return Some(super::HeuristicResult {
                action: Some(Action::Click {
                    element: el.id,
                    reasoning: format!("heuristic: e-commerce action [{}] — '{}'", el.id, el.label),
                }),
                confidence: 0.90,
                reason: format!("e-commerce button detected: '{}'", el.label),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{Element, ElementKind, PageState, SemanticView};

    fn shop_view(elements: Vec<Element>) -> SemanticView {
        SemanticView {
            url: "https://shop.example.com/product/1".into(),
            title: "Product Page".into(),
            page_hint: "content page".into(),
            elements,
            forms: vec![],
            visible_text: String::new(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    fn button_el(id: u32, label: &str) -> Element {
        Element {
            id,
            kind: ElementKind::Button,
            label: label.into(),
            name: None,
            value: None,
            placeholder: None,
            href: None,
            input_type: None,
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
            frame_index: None,
        }
    }

    fn link_el(id: u32, label: &str) -> Element {
        Element {
            id,
            kind: ElementKind::Link,
            label: label.into(),
            name: None,
            value: None,
            placeholder: None,
            href: Some("/checkout".into()),
            input_type: None,
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
            frame_index: None,
        }
    }

    #[test]
    fn add_to_cart_detected() {
        let view = shop_view(vec![button_el(0, "Add to Cart"), button_el(1, "Wishlist")]);
        let result = try_ecommerce_action(&view, "add item to cart", &[]);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.confidence >= 0.85);
        match r.action.unwrap() {
            Action::Click { element, .. } => assert_eq!(element, 0),
            other => panic!("expected Click, got {other:?}"),
        }
    }

    #[test]
    fn checkout_button_detected() {
        let view = shop_view(vec![
            link_el(0, "Proceed to Checkout"),
            button_el(1, "Continue Shopping"),
        ]);
        let result = try_ecommerce_action(&view, "checkout and pay", &[]);
        assert!(result.is_some());
        match result.unwrap().action.unwrap() {
            Action::Click { element, .. } => assert_eq!(element, 0),
            other => panic!("expected Click on checkout, got {other:?}"),
        }
    }

    #[test]
    fn no_ecommerce_goal_returns_none() {
        let view = shop_view(vec![button_el(0, "Add to Cart")]);
        let result = try_ecommerce_action(&view, "login as admin", &[]);
        assert!(result.is_none(), "non-ecommerce goal should not match");
    }

    #[test]
    fn buy_now_detected() {
        let view = shop_view(vec![button_el(0, "Buy Now"), button_el(1, "More Info")]);
        let result = try_ecommerce_action(&view, "buy this product", &[]);
        assert!(result.is_some());
        match result.unwrap().action.unwrap() {
            Action::Click { element, .. } => assert_eq!(element, 0),
            other => panic!("expected Click on Buy Now, got {other:?}"),
        }
    }
}
