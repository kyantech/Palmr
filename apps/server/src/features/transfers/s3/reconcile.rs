use crate::storage::provider::{ETag, UploadedPart};

use super::model::{S3Ref, MAX_PROVIDER_PARTS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartLayout {
    pub part_size: u64,
    pub part_count: u32,
    pub total: u64,
}

impl PartLayout {
    pub fn of(s3: &S3Ref) -> Option<Self> {
        let layout = Self {
            part_size: s3.part_size,
            part_count: s3.part_count,
            total: s3.total?.get(),
        };
        layout.is_consistent().then_some(layout)
    }

    pub fn new(part_size: u64, part_count: u64, total: u64) -> Option<Self> {
        let layout = Self {
            part_size,
            part_count: u32::try_from(part_count).ok()?,
            total,
        };
        layout.is_consistent().then_some(layout)
    }

    fn is_consistent(&self) -> bool {
        if self.part_size == 0 || self.part_count == 0 || self.part_count > MAX_PROVIDER_PARTS {
            return false;
        }
        let Some(full) = u64::from(self.part_count - 1).checked_mul(self.part_size) else {
            return false;
        };
        self.total > full && self.total - full <= self.part_size
    }

    pub fn expected(&self, number: u32) -> Option<u64> {
        if number == 0 || number > self.part_count {
            return None;
        }
        if number < self.part_count {
            return Some(self.part_size);
        }
        self.total
            .checked_sub(u64::from(self.part_count - 1).checked_mul(self.part_size)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub part_number: u32,
    pub etag: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    pub held: Vec<Held>,
    pub missing: Vec<u32>,
    pub invalid: Vec<u32>,
    pub uploaded_bytes: u64,
}

impl Analysis {
    pub fn present_numbers(&self) -> Vec<u32> {
        self.held.iter().map(|part| part.part_number).collect()
    }

    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    pub fn manifest(&self) -> Vec<UploadedPart> {
        self.held
            .iter()
            .map(|part| UploadedPart {
                part_number: part.part_number,
                etag: ETag::new(part.etag.clone()),
                size: part.size,
            })
            .collect()
    }
}

pub fn analyze(layout: &PartLayout, listed: &[UploadedPart]) -> Analysis {
    let mut ordered: Vec<&UploadedPart> = listed
        .iter()
        .filter(|part| (1..=layout.part_count).contains(&part.part_number))
        .collect();
    ordered.sort_by_key(|part| part.part_number);
    ordered.dedup_by_key(|part| part.part_number);

    let mut analysis = Analysis::default();
    let mut next = ordered.iter().peekable();
    for number in 1..=layout.part_count {
        match next.peek() {
            Some(part) if part.part_number == number => {
                let part = next.next().map(|part| (*part).clone());
                if let Some(part) = part {
                    if layout.expected(number) != Some(part.size) {
                        analysis.invalid.push(number);
                    }
                    analysis.uploaded_bytes = analysis.uploaded_bytes.saturating_add(part.size);
                    analysis.held.push(Held {
                        part_number: number,
                        etag: part.etag.as_str().to_owned(),
                        size: part.size,
                    });
                }
            }
            _ => analysis.missing.push(number),
        }
    }
    analysis
}

#[cfg(test)]
mod tests {
    use super::{analyze, PartLayout};
    use crate::storage::provider::{ETag, UploadedPart};
    use crate::storage::s3::plan::{plan_parts, PartPlan, PartPlanError};
    use crate::storage::s3::profile::{ProviderProfile, GIB, MIB, TIB};

    fn part(number: u32, size: u64) -> UploadedPart {
        UploadedPart {
            part_number: number,
            etag: ETag::new(format!("\"etag-{number}\"")),
            size,
        }
    }

    #[test]
    fn unit_layout_derives_every_part_size_from_the_plan() {
        let layout = PartLayout::new(8 * MIB, 3, 20 * MIB).unwrap();
        assert_eq!(layout.expected(1), Some(8 * MIB));
        assert_eq!(layout.expected(2), Some(8 * MIB));
        assert_eq!(layout.expected(3), Some(4 * MIB));
        assert_eq!(layout.expected(0), None);
        assert_eq!(layout.expected(4), None);
        let exact = PartLayout::new(8 * MIB, 2, 16 * MIB).unwrap();
        assert_eq!(exact.expected(2), Some(8 * MIB));
        assert!(PartLayout::new(8 * MIB, 2, 16 * MIB + 1).is_none());
        assert!(PartLayout::new(8 * MIB, 3, 16 * MIB).is_none());
        assert!(PartLayout::new(0, 1, 1).is_none());
        assert!(PartLayout::new(8 * MIB, 10_001, 1).is_none());
        assert!(PartLayout::new(u64::MAX, 3, u64::MAX).is_none());
    }

    #[test]
    fn unit_analysis_reports_missing_invalid_and_authoritative_bytes() {
        let layout = PartLayout::new(8 * MIB, 4, 30 * MIB).unwrap();
        let listed = [
            part(1, 8 * MIB),
            part(3, 5 * MIB),
            part(4, 6 * MIB),
            part(9, 1),
        ];
        let analysis = analyze(&layout, &listed);
        assert_eq!(analysis.missing, [2]);
        assert_eq!(analysis.invalid, [3]);
        assert_eq!(analysis.uploaded_bytes, 19 * MIB);
        assert_eq!(analysis.present_numbers(), [1, 3, 4]);
        assert!(!analysis.is_complete());
        assert_eq!(analysis.manifest().len(), 3);
    }

    #[test]
    fn unit_analysis_orders_and_deduplicates_the_manifest() {
        let layout = PartLayout::new(8 * MIB, 3, 20 * MIB).unwrap();
        let listed = [
            part(3, 4 * MIB),
            part(1, 8 * MIB),
            part(2, 8 * MIB),
            part(2, 8 * MIB),
        ];
        let analysis = analyze(&layout, &listed);
        assert!(analysis.is_complete());
        assert!(analysis.invalid.is_empty());
        let numbers: Vec<u32> = analysis.manifest().iter().map(|p| p.part_number).collect();
        assert_eq!(numbers, [1, 2, 3]);
        assert_eq!(analysis.uploaded_bytes, 20 * MIB);
    }

    fn layout_of(size: u64) -> Result<Option<(u64, u64)>, PartPlanError> {
        Ok(match plan_parts(size, &ProviderProfile::Minio)? {
            PartPlan::ZeroByte => None,
            PartPlan::Multipart {
                part_size,
                part_count,
            } => Some((part_size, part_count)),
        })
    }

    #[test]
    #[expect(
        non_snake_case,
        reason = "regression test names keep the upper-case catalogue identifier"
    )]
    fn regression_R001_large_object_plans_fit_the_provider_before_any_byte_moves() {
        assert_eq!(layout_of(0), Ok(None));
        assert_eq!(layout_of(5 * MIB - 1), Ok(Some((8 * MIB, 1))));
        assert_eq!(layout_of(100 * MIB), Ok(Some((8 * MIB, 13))));
        assert_eq!(layout_of(GIB), Ok(Some((8 * MIB, 128))));
        assert_eq!(layout_of(100 * GIB), Ok(Some((16 * MIB, 6_400))));
        assert_eq!(layout_of(500 * GIB), Ok(Some((64 * MIB, 8_000))));
        assert_eq!(layout_of(TIB), Ok(Some((128 * MIB, 8_192))));
        assert_eq!(layout_of(5 * TIB), Ok(Some((GIB, 5_120))));
        assert!(matches!(
            layout_of(5 * TIB + 1),
            Err(PartPlanError::FileTooLarge { .. })
        ));
        let (size, count) = layout_of(49 * GIB).unwrap().unwrap();
        assert_eq!((size, count), (8 * MIB, 6_272));
        for total in [
            49 * GIB,
            100 * GIB,
            500 * GIB,
            TIB,
            5 * TIB,
            100 * MIB,
            5 * MIB - 1,
        ] {
            let (part_size, part_count) = layout_of(total).unwrap().unwrap();
            assert!(part_count <= 9_900, "{total}");
            let layout = PartLayout::new(part_size, part_count, total).unwrap();
            let sum: u64 = (1..=layout.part_count)
                .map(|number| layout.expected(number).unwrap())
                .sum();
            assert_eq!(sum, total, "{total}");
        }
    }
}
