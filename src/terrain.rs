//! Compact in-memory sections, preserving the existing version-2 JSON cell list.
//! One hash entry per 16^3 section replaces one hash entry per block. Missing
//! cells remain unknown; growing the map never evicts the route back home.
use crate::types::Pos;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser::SerializeSeq};
use std::{collections::HashMap, fmt};

const SIDE: i32 = 16;
const VOLUME: usize = 4096;
const UNKNOWN: u32 = u32::MAX;

#[derive(Default)]
pub struct Cells {
    sections: HashMap<Pos, Box<[u32; VOLUME]>>,
    count: usize,
}
impl Cells {
    fn address(p: Pos) -> (Pos, usize) {
        let section = Pos {
            x: p.x.div_euclid(SIDE),
            y: p.y.div_euclid(SIDE),
            z: p.z.div_euclid(SIDE),
        };
        let slot =
            p.x.rem_euclid(SIDE) + SIDE * (p.z.rem_euclid(SIDE) + SIDE * p.y.rem_euclid(SIDE));
        (section, slot as usize)
    }
    pub fn insert(&mut self, p: Pos, id: u32) {
        debug_assert_ne!(id, UNKNOWN);
        let (section, slot) = Self::address(p);
        let cells = self
            .sections
            .entry(section)
            .or_insert_with(|| Box::new([UNKNOWN; VOLUME]));
        if cells[slot] == UNKNOWN {
            self.count += 1;
        }
        cells[slot] = id;
    }
    pub fn get(&self, p: &Pos) -> Option<usize> {
        let (section, slot) = Self::address(*p);
        self.sections
            .get(&section)
            .map(|cells| cells[slot])
            .filter(|id| *id != UNKNOWN)
            .map(|id| id as usize)
    }
    pub fn len(&self) -> usize {
        self.count
    }
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn storage_bytes(&self) -> usize {
        self.sections.len() * VOLUME * std::mem::size_of::<u32>()
    }
    pub fn iter(&self) -> impl Iterator<Item = (Pos, usize)> + '_ {
        self.sections.iter().flat_map(|(section, cells)| {
            cells.iter().enumerate().filter_map(move |(slot, id)| {
                if *id == UNKNOWN {
                    return None;
                }
                let slot = slot as i32;
                Some((
                    Pos {
                        x: section.x * SIDE + slot % SIDE,
                        y: section.y * SIDE + slot / (SIDE * SIDE),
                        z: section.z * SIDE + (slot / SIDE) % SIDE,
                    },
                    *id as usize,
                ))
            })
        })
    }
}
impl Serialize for Cells {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.count))?;
        for cell in self.iter() {
            sequence.serialize_element(&cell)?;
        }
        sequence.end()
    }
}
impl<'de> Deserialize<'de> for Cells {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = Cells;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a list of terrain position/palette-index pairs")
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Cells, A::Error> {
                let mut cells = Cells::default();
                while let Some((pos, id)) = seq.next_element::<(Pos, u32)>()? {
                    if id == UNKNOWN {
                        return Err(de::Error::custom("reserved terrain palette index"));
                    }
                    cells.insert(pos, id);
                }
                Ok(cells)
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn section_boundaries_negative_heights_and_unknown_cells_roundtrip() {
        let mut cells = Cells::default();
        let points = [
            Pos {
                x: -17,
                y: -65,
                z: -1,
            },
            Pos {
                x: -16,
                y: -64,
                z: 0,
            },
            Pos {
                x: -1,
                y: -1,
                z: 15,
            },
            Pos { x: 0, y: 0, z: 16 },
            Pos {
                x: 16,
                y: 16,
                z: 17,
            },
        ];
        for (id, p) in points.iter().enumerate() {
            cells.insert(*p, id as u32);
        }
        cells.insert(points[0], 99);
        assert_eq!(cells.len(), points.len());
        let restored: Cells =
            serde_json::from_slice(&serde_json::to_vec(&cells).expect("serialize")).expect("load");
        for (id, p) in points.iter().enumerate() {
            assert_eq!(restored.get(p), Some(if id == 0 { 99 } else { id }));
        }
        assert_eq!(restored.get(&Pos { x: 1, y: 0, z: 0 }), None);
        assert_eq!(restored.iter().count(), points.len());
    }
}
