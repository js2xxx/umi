pub mod dns;
pub mod tcp;
pub mod udp;

const BUFFER_CAP: usize = 16 * 1024;
const META_CAP: usize = 8;