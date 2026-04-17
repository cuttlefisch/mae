//! Help window rendering — now thin since help buffers use the normal
//! buffer_render path with rope-backed content. Only test utilities remain.

#[cfg(test)]
mod tests {
    // Tests for the body-line link parser moved to core::editor::help_ops
    // where `render_body_line` now lives.
}
