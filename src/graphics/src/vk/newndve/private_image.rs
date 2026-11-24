//! Dedicated shadow outputs. No registered guest image is ever a storage destination.
use crate::vk::{queue::{Post, Sub}, Gpu};
use ash::vk;

pub(crate) struct PrivateImage {
    dev: ash::Device,
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub generation: u64,
    memory: vk::DeviceMemory,
}

impl PrivateImage {
    pub fn new(g: &Gpu, width: u32, height: u32, format: vk::Format) -> Result<Self, String> {
        let usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE
            | vk::ImageUsageFlags::TRANSFER_SRC;
        let required = vk::FormatFeatureFlags::SAMPLED_IMAGE | vk::FormatFeatureFlags::STORAGE_IMAGE
            | vk::FormatFeatureFlags::TRANSFER_SRC | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR;
        if width == 0 || height == 0 || width > g.d.props.limits.max_image_dimension2_d
            || height > g.d.props.limits.max_image_dimension2_d || !g.d.format_features(format).contains(required) {
            return Err("NDVE private image format capabilities".into());
        }
        let ci = vk::ImageCreateInfo::default().image_type(vk::ImageType::TYPE_2D)
            .format(format).extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(1).array_layers(1).samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL).usage(usage);
        let (image, memory) = crate::vk::queue::submission_safety::create_bound_buffer(
            || unsafe { g.dev().create_image(&ci, None) }.map_err(|e| e.to_string()),
            |image| {
                let req = unsafe { g.dev().get_image_memory_requirements(*image) };
                let ty = g.d.mem_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
                    .ok_or_else(|| "NDVE private image memory type".to_string())?;
                unsafe { g.dev().allocate_memory(&vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size).memory_type_index(ty), None) }.map_err(|e| e.to_string())
            },
            |image, mem| unsafe { g.dev().bind_image_memory(*image, *mem, 0) }.map_err(|e| e.to_string()),
            |image| unsafe { g.dev().destroy_image(image, None) },
            |mem| unsafe { g.dev().free_memory(mem, None) },
        )?;
        let generation = crate::vk::image::ImageCreation::from_create_info(image, &ci).id;
        let mut owner = Self { dev: g.dev().clone(), image, memory, view: vk::ImageView::null(), generation };
        owner.view = unsafe { g.dev().create_image_view(&vk::ImageViewCreateInfo::default()
            .image(image).view_type(vk::ImageViewType::TYPE_2D).format(format)
            .subresource_range(vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1)), None)
        }.map_err(|e| e.to_string())?;
        Ok(owner)
    }

    /// Transfer every owner before recording the graph's first command.
    pub fn retain(self, sub: &mut Sub) -> vk::Image {
        let image = self.image;
        sub.post.push(Post::Callback(Box::new(move |_| drop(self))));
        image
    }

    pub fn initialize(g: &Gpu, sub: &mut Sub, image: vk::Image) {
        let barriers = [vk::ImageMemoryBarrier::default().image(image)
            .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_access_mask(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::SHADER_READ)
            .subresource_range(vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1))];
        unsafe { g.dev().cmd_pipeline_barrier(sub.main, vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::COMPUTE_SHADER, vk::DependencyFlags::empty(), &[], &[], &barriers) };
    }
}

impl Drop for PrivateImage {
    fn drop(&mut self) {
        unsafe {
            self.dev.destroy_image_view(self.view, None);
            self.dev.destroy_image(self.image, None);
            self.dev.free_memory(self.memory, None);
        }
    }
}
