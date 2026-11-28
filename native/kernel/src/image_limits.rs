use clv_abi::USER_IMAGE_MAX_BYTES;

// The build and ELF loader apply the same limit to the bundled netd image.
pub(crate) const NETD_IMAGE_MAX_BYTES: usize = if cfg!(feature = "tls-probe") {
    172 * 1024
} else {
    USER_IMAGE_MAX_BYTES
};
