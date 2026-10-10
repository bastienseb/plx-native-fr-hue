//! Fixed library header and horizontal strip regressions, over the production owned screen.
use super::*;
use super::tests::Fixture;
use plx_machine::machine::{InputEvent, Source, Tick};

fn libraries(count: usize) -> Fixture {
    let mut fixture = Fixture::new();
    let sid = plx_plex::plex::ServerId::from_raw(0);
    fixture.directory = plx_data::browse::view::DirectorySnapshot::fixture(1, 0,
        (0..count).map(|i| plx_data::browse::view::SectionView {
            sid: Some(sid), key: i as i64 + 1, kind: SecKind::Movie,
            row: plx_data::browse::SrcRow { section: i,
                title: format!("Library with a long name {i}"), pinned: i % 2 == 0,
                current: i == 0, ..Default::default() },
        }).collect());
    fixture
}

#[test]
fn every_library_including_unpinned_is_directly_reachable_without_more() {
    let _guard = plx_base::testlock::serial();
    let fixture = libraries(24);
    let page = fixture.screen();
    assert_eq!(page.libraries.len(), 24);
    assert!(!page.libraries.iter().any(|(elem, _)| *elem == MORE));
    let cx = fixture.cx(None);
    for i in 0..23 {
        assert!(matches!(page.neighbour(page.key(page.libraries[i].0), Dir::Right, &cx),
            Step::Move(key) if key.elem == page.libraries[i + 1].0));
        assert!(matches!(page.neighbour(page.key(page.libraries[i + 1].0), Dir::Left, &cx),
            Step::Move(key) if key.elem == page.libraries[i].0));
    }
}

#[test]
fn library_header_stays_fixed_and_clickable_at_the_end_of_the_catalog() {
    let _guard = plx_base::testlock::serial();
    let fixture = libraries(3);
    let mut page = fixture.screen();
    let cx = fixture.cx(None);
    let elem = page.libraries[0].0;
    let before = page.place(&elem, &cx, At::Drawn).unwrap();
    assert_eq!(before.rect.y, plx_ui::widgets::TOP_BAR_BOTTOM + plx_ui::theme::space::SM);
    page.scroll.jump(20_000.0);
    page.scroll_target = 20_000.0;
    let after = page.place(&elem, &cx, At::Drawn).unwrap();
    assert_eq!([before.rect.x, before.rect.y, before.rect.w, before.rect.h],
        [after.rect.x, after.rect.y, after.rect.w, after.rect.h]);
    assert!(after.rect.intersect(after.clip).h > 0.0);
    assert_eq!(after.clip.x, 0.0, "the fixed strip must reach the left screen edge");
    assert_eq!(after.clip.w, SCR_W, "the fixed strip must reach the right screen edge");
    let mut draw = DrawFrame::new(&cx, plx_ui::Painter::root());
    page.record_stops(&mut draw);
    assert!(draw.into_stops().iter().any(|stop| stop.key.elem == elem));
}

#[test]
fn horizontal_scroll_reveals_the_last_library_and_clips_hidden_pointer_targets() {
    let _guard = plx_base::testlock::serial();
    let fixture = libraries(24);
    let mut page = fixture.screen();
    let elem = page.libraries[23].0;
    let key = page.key(elem);
    let cx = fixture.cx(Some(key));
    let target = page.library_scroll_target(&cx, &page.library_lays(&cx));
    assert!(target > 1000.0);
    let settled = page.place(&elem, &cx, At::SpringTarget).unwrap();
    assert!(settled.rect.x >= MARGIN_X);
    assert!(settled.rect.x + settled.rect.w <= layout::GRID_RIGHT + 0.01);
    page.library_scroll.jump(target);
    let drawn = page.place(&elem, &cx, At::Drawn).unwrap();
    assert_eq!(drawn.rect.x, settled.rect.x);
    let mut draw = DrawFrame::new(&cx, plx_ui::Painter::root());
    page.record_stops(&mut draw);
    let stops = draw.into_stops();
    assert!(stops.iter().any(|stop| stop.key.elem == elem));
    assert!(!stops.iter().any(|stop| stop.key.elem == page.libraries[0].0));
}

#[test]
fn horizontal_motion_reports_damage_while_travelling_and_rests_when_settled() {
    let _guard = plx_base::testlock::serial();
    let fixture = libraries(24);
    let mut page = fixture.screen();
    let cx = fixture.cx(Some(page.key(page.libraries[23].0)));
    let target = page.library_scroll_target(&cx, &page.library_lays(&cx));
    let (_, moving) = plx_machine::idle::scoped_motion(|| page.library_scroll.step(target, K_SCROLL, 1.0 / 60.0));
    assert!(moving);
    for _ in 0..600 { page.library_scroll.step(target, K_SCROLL, 1.0 / 60.0); }
    let (_, moving) = plx_machine::idle::scoped_motion(|| page.library_scroll.step(target, K_SCROLL, 1.0 / 60.0));
    assert!(!moving, "a settled carousel must not keep requesting frames");
}

#[test]
fn back_from_deep_grid_seats_fixed_libraries_without_rewinding_the_document() {
    let _guard = plx_base::testlock::serial();
    let fixture = libraries(3);
    let mut page = fixture.screen();
    page.initial = false;
    page.scroll.jump(700.0);
    page.scroll_target = 700.0;
    let deep = page.key(page.pair.detail.elem_at(30).unwrap());
    let cx = fixture.cx(Some(deep));
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    page.step(&ScreenEvent::Input(InputEvent { at: Tick::default(), source: Source::Script,
        kind: InputKind::Key { key: Key::Back, sym: 0, wcode: 0, edge: Edge::Down, at_edge: false } }),
        &cx, &mut Effects::new(&mut out, MachineId::Instance(InstanceId(19)), &mut present));
    assert_eq!(page.scroll_target, 700.0);
    assert!(out.iter().any(|effect| matches!(&effect.fx,
        Fx::Deliver(_, Delivery::Screen(ScreenEvent::Enter(Enter::Fresh {
            focus: FocusTarget::ContainerGroup(LIBRARY_GROUP) }))))));
}

#[test]
fn grid_reveal_keeps_focused_art_clear_of_both_fixed_navigation_rows() {
    let _guard = plx_base::testlock::serial();
    let fixture = libraries(3);
    let mut page = fixture.screen();
    let key = page.key(page.pair.detail.elem_at(12).unwrap());
    page.relayout(Some(key));
    page.scroll_target = page.target_layout.row_reveal(2);
    let placed = page.place(&key.elem, &fixture.cx(Some(key)), At::SpringTarget).unwrap();
    assert!(placed.rect.y >= layout::LIBRARY_BOTTOM);
    assert!(placed.clip.y >= layout::LIBRARY_BOTTOM);
}

#[test]
fn wheel_on_a_library_moves_horizontally_without_scrolling_the_catalog() {
    let _guard = plx_base::testlock::serial();
    let fixture = libraries(3);
    let mut page = fixture.screen();
    page.scroll_target = 700.0;
    let current = page.key(page.libraries[0].0);
    let next = page.key(page.libraries[1].0);
    let mut out = Vec::new();
    let mut present = plx_machine::present::Present::new();
    page.step(&ScreenEvent::Input(InputEvent { at: Tick::default(), source: Source::Script,
        kind: InputKind::Wheel { dy: -1.0 } }), &fixture.cx(Some(current)),
        &mut Effects::new(&mut out, MachineId::Instance(InstanceId(19)), &mut present));
    assert_eq!(page.scroll_target, 700.0);
    assert!(out.iter().any(|effect| matches!(&effect.fx,
        Fx::Deliver(_, Delivery::Screen(ScreenEvent::Enter(Enter::Fresh {
            focus: FocusTarget::Elem(key) }))) if *key == next)));
}
