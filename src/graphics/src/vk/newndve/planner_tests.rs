use super::*;
use ash::vk::Handle;
use std::cell::RefCell;

fn fixture() -> Vec<PlannedJob> {
    let b = |id| info(vk::Buffer::from_raw(id), 0, 64 << 20);
    graph_jobs(GraphBuffers {
        picture: b(1), network_output: b(2), final_output: b(3), dither: b(4),
        weights: [b(5); 12], bias: [b(6); 12], resample_weights: b(7),
        picture_view: vk::ImageView::from_raw(101),
        network_sampled_view: vk::ImageView::from_raw(201),
        network_storage_view: vk::ImageView::from_raw(201),
        dither_view: vk::ImageView::from_raw(102), final_storage_view: vk::ImageView::from_raw(202),
        view_generations: [1, 2, 3, 4],
    }, &(10..18).map(b).collect::<Vec<_>>(), b(18), b(19))
}

#[test]
fn production_plan_writes_private_outputs_and_resamples_private_network() {
    let plan = fixture();
    let image_jobs: Vec<_> = plan.iter().filter_map(|(job, _, images)| images.map(|(views, _, _)| (job.kernel, views))).collect();
    assert_eq!(image_jobs.len(), 3);
    assert_eq!(image_jobs[0].0, Kernel::Out);
    assert_eq!(image_jobs[0].1[2].as_raw(), 201);
    assert_eq!(image_jobs[1].0, Kernel::Luma);
    assert_eq!(image_jobs[1].1[0].as_raw(), 201);
    assert_eq!(image_jobs[2].0, Kernel::Resample);
    assert_eq!(image_jobs[2].1.map(Handle::as_raw), [201, 102, 202]);
    for (_, views) in image_jobs { assert!([201, 202].contains(&views[2].as_raw())); }
    // Shared validator sees every actual graph job, including the image path.
    for (job, buffers, _) in plan {
        super::super::prepare::validate_buffers(buffers.map(|b| super::super::prepare::BufferSpan {
            buffer: b.buffer.as_raw(), offset: b.offset, range: b.range,
        }), job.extents().unwrap(), 256, 128 << 20, job.args()[5].div_ceil(64), 65535).unwrap();
    }
}

#[test]
fn every_actual_scratch_and_descriptor_stage_can_fail_without_issue() {
    struct Owner<'a>(usize, &'a RefCell<Vec<usize>>);
    impl Drop for Owner<'_> { fn drop(&mut self) { self.1.borrow_mut().push(self.0); } }
    // These are the actual production allocation and descriptor plans, not a
    // separate hard-coded number of stages.
    for count in [scratch_sizes().len(), fixture().len()] {
        for failure in 0..count {
            let dropped = RefCell::new(Vec::new());
            let prepared = super::super::prepare::prepare_all(0..count, |i| {
                if i == failure { Err(i) } else { Ok(Owner(i, &dropped)) }
            });
            assert!(prepared.is_err());
            let mut actual = dropped.borrow().clone(); actual.sort();
            assert_eq!(actual, (0..failure).collect::<Vec<_>>());
        }
    }
}
