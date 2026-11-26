use irori_services::gpu_hide::Adapter;

use super::ChoiceItem;
use super::controls::hide_gpu_items;

fn card(id: &str, display: bool) -> Adapter {
    Adapter {
        instance_id: id.into(),
        name: format!("Card {id}"),
        disabled: false,
        drives_display: Some(display),
    }
}

fn keys(items: Vec<ChoiceItem>) -> Vec<String> {
    items.into_iter().map(|i| i.key.to_string()).collect()
}

#[test]
fn only_cards_a_game_may_hide_are_offered() {
    let cards = [card(r"PCI\IGPU\1", false), card(r"PCI\DGPU\1", true)];
    assert_eq!(
        keys(hide_gpu_items(Some(&cards), "")),
        vec!["", r"PCI\IGPU\1"]
    );
    // The card showing the display stays visible when it is the choice.
    assert_eq!(
        keys(hide_gpu_items(Some(&cards), r"PCI\DGPU\1")),
        vec!["", r"PCI\IGPU\1", r"PCI\DGPU\1"]
    );
    assert_eq!(keys(hide_gpu_items(None, "")), vec![""]);
}
