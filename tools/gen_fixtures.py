#!/usr/bin/env python3
"""Generate small, valid AP203 STEP fixtures for tests (no external dependencies)."""
import os, sys

class W:
    def __init__(self):
        self.lines = []
        self.n = 0
    def e(self, text):
        self.n += 1
        self.lines.append(f"#{self.n}={text};")
        return self.n
    def body(self):
        return "\n".join(self.lines)

def header(name, schema="CONFIG_CONTROL_DESIGN"):
    return (f"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('stepview test fixture'),'2;1');\n"
            f"FILE_NAME('{name}','2026-09-07T12:00:00',('stepview'),(''),'gen_fixtures.py','stepview','');\n"
            f"FILE_SCHEMA(('{schema}'));\nENDSEC;\nDATA;\n")

def units(w, inch=False):
    if inch:
        mm = w.e("(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))")
        mwu = w.e(f"(CONVERSION_BASED_UNIT('INCH',#{{}})LENGTH_UNIT()NAMED_UNIT(#{{}}))")  # placeholder, fixed below
        # redo properly: measure with unit and dimensional exponents
        w.lines.pop(); w.n -= 1
        dims = w.e("DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.)")
        meas = w.e(f"LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#{mm})")
        length = w.e(f"(CONVERSION_BASED_UNIT('INCH',#{meas})LENGTH_UNIT()NAMED_UNIT(#{dims}))")
        unc_val = "3.937E-5"
    else:
        length = w.e("(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))")
        unc_val = "1.E-3"
    ang = w.e("(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))")
    sr = w.e("(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())")
    unc = w.e(f"UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE({unc_val}),#{length},'closure','')")
    ctx = w.e(f"(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc}))GLOBAL_UNIT_ASSIGNED_CONTEXT((#{length},#{ang},#{sr}))REPRESENTATION_CONTEXT('','3D'))")
    return ctx

def product(w, app, pid, name):
    pc = w.e(f"PRODUCT_CONTEXT('',#{app},'mechanical')")
    p = w.e(f"PRODUCT('{pid}','{name}','',(#{pc}))")
    pdf = w.e(f"PRODUCT_DEFINITION_FORMATION('','',#{p})")
    pdc = w.e(f"PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')")
    pd = w.e(f"PRODUCT_DEFINITION('design','',#{pdf},#{pdc})")
    pds = w.e(f"PRODUCT_DEFINITION_SHAPE('','',#{pd})")
    return pd, pds

def axis2(w, o, z, x):
    op = w.e(f"CARTESIAN_POINT('',({o[0]},{o[1]},{o[2]}))")
    zd = w.e(f"DIRECTION('',({z[0]},{z[1]},{z[2]}))")
    xd = w.e(f"DIRECTION('',({x[0]},{x[1]},{x[2]}))")
    return w.e(f"AXIS2_PLACEMENT_3D('',#{op},#{zd},#{xd})")

def cube_brep(w, size, colour=None, face_colour=None):
    s = size
    corners = [(0,0,0),(s,0,0),(s,s,0),(0,s,0),(0,0,s),(s,0,s),(s,s,s),(0,s,s)]
    pts = [w.e(f"CARTESIAN_POINT('',({c[0]},{c[1]},{c[2]}))") for c in corners]
    verts = [w.e(f"VERTEX_POINT('',#{p})") for p in pts]
    edge_defs = [(0,1),(1,2),(2,3),(3,0),(4,5),(5,6),(6,7),(7,4),(0,4),(1,5),(2,6),(3,7)]
    edges = {}
    for a,b in edge_defs:
        pa, pb = corners[a], corners[b]
        d = tuple(pb[i]-pa[i] for i in range(3))
        L = sum(x*x for x in d) ** 0.5
        dn = tuple(x/L for x in d)
        dd = w.e(f"DIRECTION('',({dn[0]},{dn[1]},{dn[2]}))")
        vec = w.e(f"VECTOR('',#{dd},{L})")
        line = w.e(f"LINE('',#{pts[a]},#{vec})")
        ec = w.e(f"EDGE_CURVE('',#{verts[a]},#{verts[b]},#{line},.T.)")
        edges[(a,b)] = ec
    # faces: (corner loop CCW seen from outside, normal, refdir)
    faces = [
        ([0,3,2,1], (0,0,-1), (1,0,0)),   # -Z
        ([4,5,6,7], (0,0,1), (1,0,0)),    # +Z
        ([0,1,5,4], (0,-1,0), (1,0,0)),   # -Y
        ([2,3,7,6], (0,1,0), (-1,0,0)),   # +Y
        ([0,4,7,3], (-1,0,0), (0,0,1)),   # -X
        ([1,2,6,5], (1,0,0), (0,1,0)),    # +X
    ]
    face_ids = []
    for loop, n, xd in faces:
        oes = []
        for i in range(4):
            a, b = loop[i], loop[(i+1)%4]
            if (a,b) in edges:
                oes.append(w.e(f"ORIENTED_EDGE('',*,*,#{edges[(a,b)]},.T.)"))
            else:
                oes.append(w.e(f"ORIENTED_EDGE('',*,*,#{edges[(b,a)]},.F.)"))
        el = w.e(f"EDGE_LOOP('',({','.join('#%d'%o for o in oes)}))")
        fob = w.e(f"FACE_OUTER_BOUND('',#{el},.T.)")
        o = corners[loop[0]]
        ax = axis2(w, o, n, xd)
        pl = w.e(f"PLANE('',#{ax})")
        face_ids.append(w.e(f"ADVANCED_FACE('',(#{fob}),#{pl},.T.)"))
    shell = w.e(f"CLOSED_SHELL('',({','.join('#%d'%f for f in face_ids)}))")
    msb = w.e(f"MANIFOLD_SOLID_BREP('cube',#{shell})")
    styled = []
    if colour:
        styled.append(style_item(w, colour, msb))
    if face_colour:
        styled.append(style_item(w, face_colour, face_ids[1]))  # +Z face override
    return msb, styled

def style_item(w, rgb, target):
    c = w.e(f"COLOUR_RGB('',{rgb[0]},{rgb[1]},{rgb[2]})")
    fasc = w.e(f"FILL_AREA_STYLE_COLOUR('',#{c})")
    fas = w.e(f"FILL_AREA_STYLE('',(#{fasc}))")
    ssfa = w.e(f"SURFACE_STYLE_FILL_AREA(#{fas})")
    sss = w.e(f"SURFACE_SIDE_STYLE('',(#{ssfa}))")
    ssu = w.e(f"SURFACE_STYLE_USAGE(.BOTH.,#{sss})")
    psa = w.e(f"PRESENTATION_STYLE_ASSIGNMENT((#{ssu}))")
    return w.e(f"STYLED_ITEM('',(#{psa}),#{target})")

def footer():
    return "\nENDSEC;\nEND-ISO-10303-21;\n"

def gen_cube_mm(path):
    w = W()
    app = w.e("APPLICATION_CONTEXT('configuration controlled 3D design of mechanical parts and assemblies')")
    w.e(f"APPLICATION_PROTOCOL_DEFINITION('international standard','config_control_design',1994,#{app})")
    ctx = units(w)
    pd, pds = product(w, app, "CUBE-10", "Cube 10mm")
    origin = axis2(w, (0,0,0), (0,0,1), (1,0,0))
    msb, styled = cube_brep(w, 10.0, colour=(0.784, 0.118, 0.118), face_colour=(0.118, 0.784, 0.118))
    absr = w.e(f"ADVANCED_BREP_SHAPE_REPRESENTATION('',(#{origin},#{msb}),#{ctx})")
    w.e(f"SHAPE_DEFINITION_REPRESENTATION(#{pds},#{absr})")
    w.e(f"MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({','.join('#%d'%s for s in styled)}),#{ctx})")
    open(path, "w").write(header("cube_mm.step") + w.body() + footer())

def gen_two_cubes_inch(path):
    """Root assembly in INCH context, child part (a 10 mm cube) in MM context, two occurrences.
    First CDSR uses rep_1 = child rep (Creo order), second uses rep_1 = parent rep (SolidWorks order)."""
    w = W()
    app = w.e("APPLICATION_CONTEXT('configuration controlled 3D design of mechanical parts and assemblies')")
    ctx_mm = units(w, inch=False)
    ctx_in = units(w, inch=True)
    # child part
    pd_c, pds_c = product(w, app, "CUBE-10", "Cube 10mm")
    origin_c = axis2(w, (0,0,0), (0,0,1), (1,0,0))
    msb, styled = cube_brep(w, 10.0, colour=(0.2, 0.4, 0.9))
    absr = w.e(f"ADVANCED_BREP_SHAPE_REPRESENTATION('',(#{origin_c},#{msb}),#{ctx_mm})")
    w.e(f"SHAPE_DEFINITION_REPRESENTATION(#{pds_c},#{absr})")
    # root assembly with two placements, in inches: second cube at x = 3 inch = 76.2 mm
    pd_a, pds_a = product(w, app, "ASM-2", "Two cubes")
    origin_a = axis2(w, (0,0,0), (0,0,1), (1,0,0))
    pos1 = axis2(w, (0,0,0), (0,0,1), (1,0,0))
    pos2 = axis2(w, (3.0,0,0), (0,0,1), (1,0,0))
    sr = w.e(f"SHAPE_REPRESENTATION('',(#{origin_a},#{pos1},#{pos2}),#{ctx_in})")
    w.e(f"SHAPE_DEFINITION_REPRESENTATION(#{pds_a},#{sr})")
    for k, (pos, creo_order) in enumerate([(pos1, True), (pos2, False)]):
        nauo = w.e(f"NEXT_ASSEMBLY_USAGE_OCCURRENCE('{k+1}','','',#{pd_a},#{pd_c},'C{k+1}')")
        pds_n = w.e(f"PRODUCT_DEFINITION_SHAPE('','',#{nauo})")
        idt = w.e(f"ITEM_DEFINED_TRANSFORMATION('','',#{origin_c},#{pos})" if creo_order else f"ITEM_DEFINED_TRANSFORMATION('','',#{pos},#{origin_c})")
        r1, r2 = (absr, sr) if creo_order else (sr, absr)
        rr = w.e(f"(REPRESENTATION_RELATIONSHIP('','',#{r1},#{r2})REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{idt})SHAPE_REPRESENTATION_RELATIONSHIP())")
        w.e(f"CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{rr},#{pds_n})")
    w.e(f"MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({','.join('#%d'%s for s in styled)}),#{ctx_mm})")
    open(path, "w").write(header("two_cubes_assembly_inch.step") + w.body() + footer())

if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "..", "tests", "fixtures")
    os.makedirs(out, exist_ok=True)
    gen_cube_mm(os.path.join(out, "cube_mm.step"))
    gen_two_cubes_inch(os.path.join(out, "two_cubes_assembly_inch.step"))
    print("wrote fixtures to", out)
