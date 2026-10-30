#version 330 core
in vec4 a_position;
in vec4 a_bounds;
in vec4 a_mask;
in vec4 a_color;
in vec4 a_border;
in vec4 a_radii;
in vec4 a_widths;
in vec4 a_other;
in vec4 a_paint;
in vec4 a_params;
in vec4 a_element;
in vec4 a_element_radii;
uniform vec2 viewport;
in vec4 a_axis_x;
in vec4 a_axis_y;
out vec2 position;
out vec2 uv;
out vec4 bounds;
out vec4 mask;
out vec4 color;
out vec4 border;
out vec4 radii;
out vec4 widths;
out vec4 other;
out vec4 paint;
out vec4 params;
out vec4 element;
out vec4 element_radii;
void main() {
    vec4 p=a_position;
    // Each instance supplies one affine triangle or rectangle. The first
    // three corners also cover an arbitrary path triangle.
    int i=gl_VertexID;
    vec2 corner=(i==1||i==4)?vec2(1.0,0.0):(i==2||i==3)?vec2(0.0,1.0):i==5?vec2(1.0,1.0):vec2(0.0);
    p+=a_axis_x*corner.x+a_axis_y*corner.y;
    gl_Position=vec4(p.x/viewport.x*2.0-1.0,1.0-p.y/viewport.y*2.0,0.0,1.0);
    position=p.xy; uv=p.zw;
    bounds=a_bounds; mask=a_mask; color=a_color; border=a_border;
    radii=a_radii; widths=a_widths; other=a_other; paint=a_paint;
    params=a_params; element=a_element; element_radii=a_element_radii;
}
