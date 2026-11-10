//! Component selection helpers: grow/shrink, conversion, edge loops/rings, borders.

use crate::mesh::{edge_loop, edge_ring, epair, PolyMesh, Topo};
use crate::scene::{CompMode, CompSel, Scene};
use std::collections::BTreeSet;

/// Convert the component selection of every hilited mesh to another mode.
pub fn convert(s: &mut Scene, to: CompMode) {
    for id in s.hilite() {
        let Some(pm) = s.node(&id).and_then(|n| n.mesh()).map(|m| m.mesh.clone()) else { continue };
        let c = s.comp_sel(&id);
        let topo = pm.topo();
        let verts: BTreeSet<u32> = s.comp_verts(&id).into_iter().collect();
        let mut out = CompSel::default();
        match to {
            CompMode::Vertex => out.v = verts,
            CompMode::Edge => {
                if !c.f.is_empty() {
                    for &f in &c.f {
                        let face = &pm.f[f];
                        for i in 0..face.len() {
                            out.e.insert(epair(face[i], face[(i + 1) % face.len()]));
                        }
                    }
                } else {
                    for &(a, b) in &topo.edges {
                        if verts.contains(&a) && verts.contains(&b) {
                            out.e.insert((a, b));
                        }
                    }
                    // JS `convertComp`: vertices with no complete edge select their incident edges
                    if out.e.is_empty() {
                        for &(a, b) in &topo.edges {
                            if verts.contains(&a) || verts.contains(&b) {
                                out.e.insert((a, b));
                            }
                        }
                    }
                }
            }
            CompMode::Face => {
                if !c.e.is_empty() {
                    // JS: the faces of the selected edges
                    for (fi, f) in pm.f.iter().enumerate() {
                        if (0..f.len()).any(|i| c.e.contains(&epair(f[i], f[(i + 1) % f.len()]))) {
                            out.f.insert(fi);
                        }
                    }
                } else {
                    for (fi, f) in pm.f.iter().enumerate() {
                        if f.iter().all(|v| verts.contains(v)) {
                            out.f.insert(fi);
                        }
                    }
                    // JS fallback: a lone vertex selects the faces it belongs to
                    if out.f.is_empty() {
                        for (fi, f) in pm.f.iter().enumerate() {
                            if f.iter().any(|v| verts.contains(v)) {
                                out.f.insert(fi);
                            }
                        }
                    }
                }
            }
        }
        s.comp.insert(id, out);
    }
    s.comp_mode = Some(to);
    s.rev += 1;
}

/// Grow (or shrink) the selection by one ring of neighbours.
pub fn grow(s: &mut Scene, shrink: bool) {
    let Some(mode) = s.comp_mode else { return };
    for id in s.hilite() {
        let Some(pm) = s.node(&id).and_then(|n| n.mesh()).map(|m| m.mesh.clone()) else { continue };
        let topo = pm.topo();
        let c = s.comp_sel(&id);
        let mut out = c.clone();
        match mode {
            CompMode::Vertex => {
                if shrink {
                    // keep vertices whose neighbours are all selected
                    out.v = c.v.iter().copied().filter(|&v| topo.vert_edges[v as usize].iter().all(|&e| {
                        let (a, b) = topo.edges[e];
                        c.v.contains(&a) && c.v.contains(&b)
                    })).collect();
                } else {
                    for &v in &c.v {
                        for &f in &topo.vert_faces[v as usize] {
                            out.v.extend(pm.f[f].iter().copied());
                        }
                    }
                }
            }
            CompMode::Face => {
                if shrink {
                    out.f = c.f.iter().copied().filter(|&f| pm.f[f].iter().all(|v| topo.vert_faces[*v as usize].iter().all(|g| c.f.contains(g)))).collect();
                } else {
                    for &f in &c.f {
                        for v in &pm.f[f] {
                            out.f.extend(topo.vert_faces[*v as usize].iter().copied());
                        }
                    }
                }
            }
            CompMode::Edge => {
                if shrink {
                    out.e = c.e.iter().copied().filter(|&(a, b)| [a, b].iter().all(|v| topo.vert_edges[*v as usize].iter().all(|e| c.e.contains(&topo.edges[*e])))).collect();
                } else {
                    for &(a, b) in &c.e {
                        for v in [a, b] {
                            out.e.extend(topo.vert_edges[v as usize].iter().map(|&e| topo.edges[e]));
                        }
                    }
                }
            }
        }
        s.comp.insert(id, out);
    }
    s.rev += 1;
}

/// The face loop through two neighbouring faces: the quads strung along the edge ring that
/// crosses their shared edge. None when the faces don't share an edge.
pub fn face_loop(pm: &PolyMesh, topo: &Topo, fa: usize, fb: usize) -> Option<Vec<usize>> {
    let (a, b) = pm.f.get(fa)?.iter().enumerate().map(|(i, &v)| (v, pm.f[fa][(i + 1) % pm.f[fa].len()])).find(|&(x, y)| topo.edge(x, y).is_some_and(|e| topo.edge_faces[e].contains(&fb)))?;
    let ring: BTreeSet<(u32, u32)> = edge_ring(pm, topo, a, b).into_iter().map(|(x, y)| epair(x, y)).collect();
    let mut faces: BTreeSet<usize> = [fa, fb].into_iter().collect();
    for e in &ring {
        let Some(ei) = topo.edge(e.0, e.1) else { continue };
        for &f in &topo.edge_faces[ei] {
            let face = &pm.f[f];
            let n = (0..face.len()).filter(|&i| ring.contains(&epair(face[i], face[(i + 1) % face.len()]))).count();
            if face.len() == 4 && n >= 2 {
                faces.insert(f);
            }
        }
    }
    Some(faces.into_iter().collect())
}

/// Straight path of quads between two faces on the same face loop (the faces between them).
/// Used for Shift+double-click on a non-adjacent face.
pub fn face_path(pm: &PolyMesh, topo: &Topo, fa: usize, fb: usize) -> Option<Vec<usize>> {
    if fa == fb {
        return Some(vec![fa]);
    }
    if pm.f.get(fa).is_none_or(|f| f.len() != 4) {
        return None;
    }
    let shared = |fa: usize, fb: usize| {
        let face = pm.f.get(fa)?;
        (0..face.len()).map(|i| (face[i], face[(i + 1) % face.len()])).find(|&(a, b)| topo.edge(a, b).is_some_and(|e| topo.edge_faces[e].contains(&fb)))
    };
    let mut best: Option<Vec<usize>> = None;
    for i in 0..4 {
        let (a, b) = (pm.f[fa][i], pm.f[fa][(i + 1) % 4]);
        let Some(e) = topo.edge(a, b) else { continue };
        let Some(first) = topo.edge_faces[e].iter().copied().find(|&f| f != fa) else { continue };
        let mut path = vec![fa, first];
        if first == fb {
            if best.as_ref().is_none_or(|p| path.len() < p.len()) {
                best = Some(path);
            }
            continue;
        }
        let mut prev = fa;
        let mut cur = first;
        loop {
            let face = &pm.f[cur];
            if face.len() != 4 {
                break;
            }
            let Some((sa, sb)) = shared(cur, prev) else { break };
            let Some(i) = (0..4).find(|&i| epair(face[i], face[(i + 1) % 4]) == epair(sa, sb)) else { break };
            let Some(next_edge) = topo.edge(face[(i + 2) % 4], face[(i + 3) % 4]) else { break };
            let Some(&next) = topo.edge_faces[next_edge].iter().find(|&&f| f != cur) else { break };
            if path.contains(&next) {
                break;
            }
            path.push(next);
            if next == fb {
                if best.as_ref().is_none_or(|p| path.len() < p.len()) {
                    best = Some(path);
                }
                break;
            }
            prev = cur;
            cur = next;
        }
    }
    best
}

/// Add the face loop through `face` and a selected face (Shift+double-click a face next to a
/// selected one; non-adjacent faces select the partial loop between them). Returns false when
/// there is no selected face on the same loop.
pub fn add_face_loop(s: &mut Scene, id: &str, face: usize) -> bool {
    let Some(pm) = s.node(id).and_then(|n| n.mesh()).map(|m| m.mesh.clone()) else { return false };
    let topo = pm.topo();
    let sel = s.comp_sel(id);
    let selected: Vec<usize> = sel.f.iter().copied().filter(|&f| f != face).collect();
    if selected.is_empty() {
        return false;
    }
    let lp = selected.iter().find_map(|&f| face_loop(&pm, &topo, f, face)).or_else(|| selected.iter().filter_map(|&f| face_path(&pm, &topo, f, face)).min_by_key(|p| p.len()));
    let Some(lp) = lp else { return false };
    let mut out = sel.clone();
    out.f.extend(lp);
    s.comp.insert(id.to_string(), out);
    s.comp_mode = Some(CompMode::Face);
    s.rev += 1;
    true
}

/// Extend selected edges to full loops (`ring` = edge rings).
pub fn loops(s: &mut Scene, ring: bool) {
    for id in s.hilite() {
        let Some(pm) = s.node(&id).and_then(|n| n.mesh()).map(|m| m.mesh.clone()) else { continue };
        let topo = pm.topo();
        let c = s.comp_sel(&id);
        let mut out = c.clone();
        for &(a, b) in &c.e {
            if ring {
                out.e.extend(edge_ring(&pm, &topo, a, b).into_iter().map(|(x, y)| epair(x, y)));
            } else {
                out.e.extend(edge_loop(&pm, &topo, a, b));
            }
        }
        s.comp.insert(id, out);
    }
    s.comp_mode = Some(CompMode::Edge);
    s.rev += 1;
}

/// Select border edges of the hilited meshes.
pub fn border(s: &mut Scene) {
    for id in s.hilite() {
        let Some(pm) = s.node(&id).and_then(|n| n.mesh()).map(|m| m.mesh.clone()) else { continue };
        let topo = pm.topo();
        let e: BTreeSet<(u32, u32)> = topo.edges.iter().enumerate().filter(|(i, _)| topo.edge_faces[*i].len() == 1).map(|(_, e)| *e).collect();
        s.comp.insert(id, CompSel { e, ..Default::default() });
    }
    s.comp_mode = Some(CompMode::Edge);
    s.rev += 1;
}

/// Select all / invert components of the current mode.
pub fn all_components(s: &mut Scene, invert: bool) {
    let Some(mode) = s.comp_mode else { return };
    for id in s.hilite() {
        let c = s.comp_sel(&id);
        if let Some(len) = s.node(&id).and_then(|n| n.curve()).map(|c| c.cvs.len()) {
            // curves only have CVs; `e[*]` / `f[*]` in the JS would be empty too
            if mode == CompMode::Vertex {
                let v = (0..len as u32).filter(|v| !invert || !c.v.contains(v)).collect();
                s.comp.insert(id, CompSel { v, ..Default::default() });
            }
            continue;
        }
        let Some(pm) = s.node(&id).and_then(|n| n.mesh()).map(|m| m.mesh.clone()) else { continue };
        let mut out = CompSel::default();
        match mode {
            CompMode::Vertex => out.v = (0..pm.v.len() as u32).filter(|v| !invert || !c.v.contains(v)).collect(),
            CompMode::Face => out.f = (0..pm.f.len()).filter(|f| !invert || !c.f.contains(f)).collect(),
            CompMode::Edge => out.e = pm.topo().edges.into_iter().filter(|e| !invert || !c.e.contains(e)).collect(),
        }
        s.comp.insert(id, out);
    }
    s.rev += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;

    #[test]
    fn loop_and_convert() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        s.select(std::slice::from_ref(&id), crate::scene::SelMode::Replace);
        s.set_comp_mode(Some(CompMode::Edge));
        let e = s.node(&id).unwrap().mesh().unwrap().mesh.topo().edges[0];
        s.comp.insert(id.clone(), CompSel { e: [e].into_iter().collect(), ..Default::default() });
        loops(&mut s, true);
        assert_eq!(s.comp_sel(&id).e.len(), 4);
        convert(&mut s, CompMode::Vertex);
        assert_eq!(s.comp_sel(&id).v.len(), 8);
        convert(&mut s, CompMode::Face);
        assert_eq!(s.comp_sel(&id).f.len(), 6);
        grow(&mut s, true);
        assert_eq!(s.comp_sel(&id).f.len(), 6);
    }

    #[test]
    fn set_comp_mode_converts_and_keeps_the_selection() {
        let mut s = Scene::new();
        let id = s.create_mesh("polyCube", &Params::new(), None);
        s.select(std::slice::from_ref(&id), crate::scene::SelMode::Replace);
        s.set_comp_mode(Some(CompMode::Vertex));
        let face0 = s.node(&id).unwrap().mesh().unwrap().mesh.f[0].clone();
        s.comp.insert(id.clone(), CompSel { v: face0.iter().copied().collect(), ..Default::default() });
        // switching component types converts (JS `Sel.setMode`), it does not drop the selection
        s.set_comp_mode(Some(CompMode::Face));
        assert_eq!(s.comp_sel(&id).f.iter().copied().collect::<Vec<_>>(), vec![0]);
        // the per-object component sets survive a trip through object mode
        s.set_comp_mode(None);
        assert!(s.comp_mode.is_none());
        assert!(!s.comp_sel(&id).f.is_empty());
        s.set_comp_mode(Some(CompMode::Face));
        assert_eq!(s.comp_sel(&id).f.iter().copied().collect::<Vec<_>>(), vec![0]);
        // a lone vertex converts to the faces around it (JS fallback)
        s.set_comp_mode(Some(CompMode::Vertex));
        s.comp.insert(id.clone(), CompSel { v: [face0[0]].into_iter().collect(), ..Default::default() });
        s.set_comp_mode(Some(CompMode::Face));
        assert!(s.comp_sel(&id).f.len() >= 1);
    }

    #[test]
    fn face_loop_from_two_neighbours() {
        let mut s = Scene::new();
        // a 4x4 plane: a face loop is one row of four faces
        let mut p = Params::new();
        p.insert("subdivisionsWidth".into(), serde_json::json!(4));
        p.insert("subdivisionsHeight".into(), serde_json::json!(4));
        let id = s.create_mesh("polyPlane", &p, None);
        s.select(std::slice::from_ref(&id), crate::scene::SelMode::Replace);
        s.set_comp_mode(Some(CompMode::Face));
        assert_eq!(s.node(&id).unwrap().mesh().unwrap().mesh.f.len(), 16);
        s.comp.insert(id.clone(), CompSel { f: [5].into_iter().collect(), ..Default::default() });
        // 6 is beside 5: the row 4..8
        assert!(add_face_loop(&mut s, &id, 6));
        assert_eq!(s.comp_sel(&id).f.iter().copied().collect::<Vec<_>>(), vec![4, 5, 6, 7]);
        // 9 is above 5: adds the column 1, 5, 9, 13
        s.comp.insert(id.clone(), CompSel { f: [5].into_iter().collect(), ..Default::default() });
        assert!(add_face_loop(&mut s, &id, 9));
        assert_eq!(s.comp_sel(&id).f.iter().copied().collect::<Vec<_>>(), vec![1, 5, 9, 13]);
        // non-adjacent faces select the partial loop between them: 4 -> 6 is 4, 5, 6
        s.comp.insert(id.clone(), CompSel { f: [4].into_iter().collect(), ..Default::default() });
        assert!(add_face_loop(&mut s, &id, 6));
        assert_eq!(s.comp_sel(&id).f.iter().copied().collect::<Vec<_>>(), vec![4, 5, 6]);
        // perpendicular partial loop: 1 -> 13 is 1, 5, 9, 13
        s.comp.insert(id.clone(), CompSel { f: [1].into_iter().collect(), ..Default::default() });
        assert!(add_face_loop(&mut s, &id, 13));
        assert_eq!(s.comp_sel(&id).f.iter().copied().collect::<Vec<_>>(), vec![1, 5, 9, 13]);
        // not a neighbour: nothing
        s.comp.insert(id.clone(), CompSel { f: [0].into_iter().collect(), ..Default::default() });
        assert!(!add_face_loop(&mut s, &id, 15));
        // a cube: the four faces around
        let cube = s.create_mesh("polyCube", &Params::new(), None);
        let pm = s.node(&cube).unwrap().mesh().unwrap().mesh.clone();
        let topo = pm.topo();
        let (f0, f1) = (0..6).flat_map(|a| (0..6).map(move |b| (a, b))).find(|&(a, b)| a != b && face_loop(&pm, &topo, a, b).is_some()).unwrap();
        assert_eq!(face_loop(&pm, &topo, f0, f1).unwrap().len(), 4);
    }
}
