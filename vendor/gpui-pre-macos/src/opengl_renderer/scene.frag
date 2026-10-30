#version 330 core
out vec4 fragment_color;
uniform sampler2D atlas;
in vec2 position;
in vec2 uv;
in vec4 bounds;
in vec4 mask;
in vec4 color;
in vec4 border;
in vec4 radii;
in vec4 widths;
in vec4 other;
in vec4 paint;
in vec4 params;
in vec4 element;
in vec4 element_radii;

vec4 premultiply(vec4 c) {return vec4(c.rgb*c.a,c.a);}
float distanceRect(vec2 p,vec4 b,vec4 r) {
    vec2 q=p-b.xy-b.zw*0.5;
    float radius=q.x>0.0?(q.y>0.0?r.z:r.y):(q.y>0.0?r.w:r.x);
    radius=clamp(radius,0.0,min(b.z,b.w)*0.5);
    vec2 d=abs(q)-b.zw*0.5+radius;
    return length(max(d,0.0))+min(max(d.x,d.y),0.0)-radius;
}
float coverage(vec2 p,vec4 b,vec4 r) {
    if (b.z<=0.0||b.w<=0.0) return 0.0;
    return clamp(0.5-distanceRect(p,b,r),0.0,1.0);
}
vec3 linearRgb(vec3 c) {
    return mix(c/12.92,pow((c+0.055)/1.055,vec3(2.4)),step(vec3(0.04045),c));
}
vec3 srgb(vec3 c) {
    c=max(c,0.0);
    return mix(c*12.92,1.055*pow(c,vec3(1.0/2.4))-0.055,step(vec3(0.0031308),c));
}
vec3 oklab(vec3 c) {
    vec3 lms=vec3(dot(c,vec3(0.4122214708,0.5363325363,0.0514459929)),dot(c,vec3(0.2119034982,0.6806995451,0.1073969566)),dot(c,vec3(0.0883024619,0.2817188376,0.6299787005)));
    lms=pow(max(lms,0.0),vec3(1.0/3.0));
    return vec3(dot(lms,vec3(0.2104542553,0.7936177850,-0.0040720468)),dot(lms,vec3(1.9779984951,-2.4285922050,0.4505937099)),dot(lms,vec3(0.0259040371,0.7827717662,-0.8086757660)));
}
vec3 fromOklab(vec3 c) {
    vec3 lms=vec3(c.x+0.3963377774*c.y+0.2158037573*c.z,c.x-0.1055613458*c.y-0.0638541728*c.z,c.x-0.0894841775*c.y-1.2914855480*c.z);
    lms=lms*lms*lms;
    return vec3(dot(lms,vec3(4.0767416621,-3.3077115913,0.2309699292)),dot(lms,vec3(-1.2684380046,2.6097574011,-0.3413193965)),dot(lms,vec3(-0.0041960863,-0.7034186147,1.7076147010)));
}
vec4 fillColor() {
    if (params.y<0.5) return color;
    if (params.y<1.5) {
        float angle=(paint.x-90.0)*0.01745329252;
        vec2 direction=vec2(cos(angle),sin(angle));
        if(bounds.z>bounds.w)direction.y*=bounds.w/bounds.z;
        else direction.x*=bounds.z/bounds.w;
        float t=dot(position-bounds.xy-bounds.zw*0.5,normalize(direction));
        t=t/(abs(direction.x)>abs(direction.y)?bounds.z:bounds.w)+0.5;
        t=abs(paint.z-paint.y)<0.000001?step(paint.z,t):clamp((t-paint.y)/(paint.z-paint.y),0.0,1.0);
        vec4 c=mix(color,other,t);
        if(paint.w>0.5)c.rgb=srgb(fromOklab(mix(oklab(linearRgb(color.rgb)),oklab(linearRgb(other.rgb)),t)));
        return c;
    }
    vec2 p=position-bounds.xy;
    vec4 c=color;
    if(params.y>2.5)c.a*=mod(floor(p.x/max(paint.x,1.0))+floor(p.y/max(paint.x,1.0)),2.0);
    else {
        float w=(paint.x/65535.0)/255.0;
        float interval=mod(paint.x,65535.0)/255.0;
        float period=(w+interval)*0.70710678118;
        float phase=mod((p.x+p.y)*0.70710678118,max(period,0.000001));
        c.a*=period>0.0?clamp(0.5-(min(phase,period-phase)-w*0.35355339059),0.0,1.0):0.0;
    }
    return c;
}
void main() {
    vec2 lo=max(position-0.5,mask.xy);
    vec2 hi=min(position+0.5,mask.xy+mask.zw);
    vec2 covered=max(hi-lo,0.0);
    float clip=covered.x*covered.y;
    if(clip<=0.0)discard;
    vec4 c;
    if(params.x<0.5) {
        float outer=coverage(position,bounds,radii);
        vec4 inner=vec4(bounds.xy+vec2(widths.w,widths.x),bounds.zw-vec2(widths.w+widths.y,widths.x+widths.z));
        vec4 ir=max(radii-vec4(max(widths.w,widths.x),max(widths.y,widths.x),max(widths.y,widths.z),max(widths.w,widths.z)),0.0);
        float amount=outer>0.0?clamp((outer-coverage(position,inner,ir))/outer,0.0,1.0):0.0;
        if(params.z>0.5) {
            vec2 p=position-bounds.xy;
            float edge=min(min(p.x,bounds.z-p.x),min(p.y,bounds.w-p.y));
            float along=(edge==p.x||edge==bounds.z-p.x)?p.y:p.x;
            float thickness=max(max(widths.x,widths.y),max(widths.z,widths.w));
            amount*=step(mod(along,max(thickness*6.0,1.0)),max(thickness*3.0,0.5));
        }
        vec4 bg=premultiply(fillColor());
        vec4 stroke=premultiply(border);
        c=mix(bg,stroke+bg*(1.0-stroke.a),amount)*outer;
    } else if(params.x<2.5) {
        vec4 sampled=texture(atlas,uv);
        if(params.x<1.5)c=premultiply(color)*sampled.a;
        else {
            if(params.z>0.5)sampled.rgb=vec3(dot(sampled.rgb,vec3(0.2126,0.7152,0.0722)));
            c=sampled*params.w*coverage(position,bounds,radii);
        }
    } else if(params.x<3.5) {
        float edge=uv.y-uv.x*uv.x;
        c=premultiply(fillColor())*clamp(edge/max(fwidth(edge),0.000001)+0.5,0.0,1.0);
    } else if(params.x<4.5) {
        float d=distanceRect(position,bounds,radii);
        float blur=max(params.z,0.01);
        float alpha=1.0/(1.0+exp(clamp(d/(blur*0.5),-40.0,40.0)));
        float inside=coverage(position,element,element_radii);
        alpha=params.w>0.5?(1.0-alpha)*inside:alpha*(1.0-inside);
        c=premultiply(color)*alpha;
    } else {
        float center=bounds.y+params.z*0.5;
        if(params.w>0.5)center+=sin((position.x-bounds.x)*6.28318530718/max(params.z*4.0,1.0))*params.z;
        c=premultiply(color)*clamp(params.z*0.5+0.5-abs(position.y-center),0.0,1.0);
    }
    fragment_color=c*clip;
}
