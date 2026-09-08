//! Entity type names the viewer knows about. Everything else is [`EntityType::Other`] (its name is
//! still counted and retrievable from the file bytes).

macro_rules! define_entity_types {
    ($( $variant:ident = $name:literal ),* $(,)?) => {
        /// Known STEP entity types (a subset of AP203/AP214/AP242 sufficient for viewing).
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u16)]
        pub enum EntityType {
            /// Empty slot in the entity table (no entity with this id).
            #[default]
            Missing = 0,
            /// A simple entity whose type is not in the known list.
            Other,
            /// A complex (multi-supertype) instance; see `StepFile::complex_parts`.
            Complex,
            $( $variant, )*
        }

        static NAME_MAP: phf::Map<&'static str, EntityType> = phf::phf_map! {
            $( $name => EntityType::$variant, )*
        };

        impl EntityType {
            /// Look up a known type by its upper-case name.
            #[inline]
            pub fn from_name(name: &[u8]) -> EntityType {
                match std::str::from_utf8(name) {
                    Ok(s) => NAME_MAP.get(s).copied().unwrap_or(EntityType::Other),
                    Err(_) => EntityType::Other,
                }
            }

            /// Canonical name (placeholder for the sentinels).
            pub fn name(self) -> &'static str {
                match self {
                    EntityType::Missing => "",
                    EntityType::Other => "<other>",
                    EntityType::Complex => "<complex>",
                    $( EntityType::$variant => $name, )*
                }
            }

            /// All known (non-sentinel) types.
            pub const ALL: &'static [EntityType] = &[ $( EntityType::$variant, )* ];
        }
    };
}

define_entity_types! {
    // --- geometry ---
    CartesianPoint = "CARTESIAN_POINT",
    Direction = "DIRECTION",
    Vector = "VECTOR",
    Axis1Placement = "AXIS1_PLACEMENT",
    Axis2Placement2d = "AXIS2_PLACEMENT_2D",
    Axis2Placement3d = "AXIS2_PLACEMENT_3D",
    CartesianTransformationOperator3d = "CARTESIAN_TRANSFORMATION_OPERATOR_3D",
    ItemDefinedTransformation = "ITEM_DEFINED_TRANSFORMATION",
    Line = "LINE",
    Circle = "CIRCLE",
    Ellipse = "ELLIPSE",
    Hyperbola = "HYPERBOLA",
    Parabola = "PARABOLA",
    Polyline = "POLYLINE",
    BSplineCurve = "B_SPLINE_CURVE",
    BSplineCurveWithKnots = "B_SPLINE_CURVE_WITH_KNOTS",
    BezierCurve = "BEZIER_CURVE",
    QuasiUniformCurve = "QUASI_UNIFORM_CURVE",
    UniformCurve = "UNIFORM_CURVE",
    RationalBSplineCurve = "RATIONAL_B_SPLINE_CURVE",
    BoundedCurve = "BOUNDED_CURVE",
    TrimmedCurve = "TRIMMED_CURVE",
    CompositeCurve = "COMPOSITE_CURVE",
    CompositeCurveSegment = "COMPOSITE_CURVE_SEGMENT",
    Pcurve = "PCURVE",
    SurfaceCurve = "SURFACE_CURVE",
    SeamCurve = "SEAM_CURVE",
    IntersectionCurve = "INTERSECTION_CURVE",
    Curve = "CURVE",
    Surface = "SURFACE",
    BoundedSurface = "BOUNDED_SURFACE",
    Plane = "PLANE",
    CylindricalSurface = "CYLINDRICAL_SURFACE",
    ConicalSurface = "CONICAL_SURFACE",
    SphericalSurface = "SPHERICAL_SURFACE",
    ToroidalSurface = "TOROIDAL_SURFACE",
    DegenerateToroidalSurface = "DEGENERATE_TOROIDAL_SURFACE",
    SurfaceOfLinearExtrusion = "SURFACE_OF_LINEAR_EXTRUSION",
    SurfaceOfRevolution = "SURFACE_OF_REVOLUTION",
    BSplineSurface = "B_SPLINE_SURFACE",
    BSplineSurfaceWithKnots = "B_SPLINE_SURFACE_WITH_KNOTS",
    BezierSurface = "BEZIER_SURFACE",
    QuasiUniformSurface = "QUASI_UNIFORM_SURFACE",
    UniformSurface = "UNIFORM_SURFACE",
    RationalBSplineSurface = "RATIONAL_B_SPLINE_SURFACE",
    RectangularTrimmedSurface = "RECTANGULAR_TRIMMED_SURFACE",
    CurveBoundedSurface = "CURVE_BOUNDED_SURFACE",
    OffsetSurface = "OFFSET_SURFACE",
    GeometricRepresentationItem = "GEOMETRIC_REPRESENTATION_ITEM",
    RepresentationItem = "REPRESENTATION_ITEM",
    // --- topology ---
    VertexPoint = "VERTEX_POINT",
    EdgeCurve = "EDGE_CURVE",
    OrientedEdge = "ORIENTED_EDGE",
    EdgeLoop = "EDGE_LOOP",
    VertexLoop = "VERTEX_LOOP",
    PolyLoop = "POLY_LOOP",
    FaceBound = "FACE_BOUND",
    FaceOuterBound = "FACE_OUTER_BOUND",
    AdvancedFace = "ADVANCED_FACE",
    FaceSurface = "FACE_SURFACE",
    OrientedFace = "ORIENTED_FACE",
    ClosedShell = "CLOSED_SHELL",
    OpenShell = "OPEN_SHELL",
    OrientedClosedShell = "ORIENTED_CLOSED_SHELL",
    OrientedOpenShell = "ORIENTED_OPEN_SHELL",
    ConnectedFaceSet = "CONNECTED_FACE_SET",
    ManifoldSolidBrep = "MANIFOLD_SOLID_BREP",
    BrepWithVoids = "BREP_WITH_VOIDS",
    FacetedBrep = "FACETED_BREP",
    ShellBasedSurfaceModel = "SHELL_BASED_SURFACE_MODEL",
    GeometricSet = "GEOMETRIC_SET",
    GeometricCurveSet = "GEOMETRIC_CURVE_SET",
    MappedItem = "MAPPED_ITEM",
    RepresentationMap = "REPRESENTATION_MAP",
    // --- representations ---
    Representation = "REPRESENTATION",
    ShapeRepresentation = "SHAPE_REPRESENTATION",
    AdvancedBrepShapeRepresentation = "ADVANCED_BREP_SHAPE_REPRESENTATION",
    ManifoldSurfaceShapeRepresentation = "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
    FacetedBrepShapeRepresentation = "FACETED_BREP_SHAPE_REPRESENTATION",
    GeometricallyBoundedSurfaceShapeRepresentation = "GEOMETRICALLY_BOUNDED_SURFACE_SHAPE_REPRESENTATION",
    GeometricallyBoundedWireframeShapeRepresentation = "GEOMETRICALLY_BOUNDED_WIREFRAME_SHAPE_REPRESENTATION",
    EdgeBasedWireframeShapeRepresentation = "EDGE_BASED_WIREFRAME_SHAPE_REPRESENTATION",
    ShapeRepresentationRelationship = "SHAPE_REPRESENTATION_RELATIONSHIP",
    RepresentationRelationship = "REPRESENTATION_RELATIONSHIP",
    RepresentationRelationshipWithTransformation = "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION",
    ContextDependentShapeRepresentation = "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
    ShapeDefinitionRepresentation = "SHAPE_DEFINITION_REPRESENTATION",
    PropertyDefinitionRepresentation = "PROPERTY_DEFINITION_REPRESENTATION",
    PropertyDefinition = "PROPERTY_DEFINITION",
    GeometricRepresentationContext = "GEOMETRIC_REPRESENTATION_CONTEXT",
    GlobalUnitAssignedContext = "GLOBAL_UNIT_ASSIGNED_CONTEXT",
    GlobalUncertaintyAssignedContext = "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT",
    RepresentationContext = "REPRESENTATION_CONTEXT",
    ParametricRepresentationContext = "PARAMETRIC_REPRESENTATION_CONTEXT",
    // --- product structure ---
    Product = "PRODUCT",
    ProductDefinition = "PRODUCT_DEFINITION",
    ProductDefinitionFormation = "PRODUCT_DEFINITION_FORMATION",
    ProductDefinitionFormationWithSpecifiedSource = "PRODUCT_DEFINITION_FORMATION_WITH_SPECIFIED_SOURCE",
    ProductDefinitionShape = "PRODUCT_DEFINITION_SHAPE",
    ProductDefinitionContext = "PRODUCT_DEFINITION_CONTEXT",
    ProductContext = "PRODUCT_CONTEXT",
    ProductRelatedProductCategory = "PRODUCT_RELATED_PRODUCT_CATEGORY",
    ProductCategory = "PRODUCT_CATEGORY",
    ApplicationContext = "APPLICATION_CONTEXT",
    ApplicationProtocolDefinition = "APPLICATION_PROTOCOL_DEFINITION",
    NextAssemblyUsageOccurrence = "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
    AssemblyComponentUsage = "ASSEMBLY_COMPONENT_USAGE",
    ProductDefinitionUsage = "PRODUCT_DEFINITION_USAGE",
    ProductDefinitionRelationship = "PRODUCT_DEFINITION_RELATIONSHIP",
    ShapeAspect = "SHAPE_ASPECT",
    ShapeAspectRelationship = "SHAPE_ASPECT_RELATIONSHIP",
    DesignContext = "DESIGN_CONTEXT",
    MechanicalContext = "MECHANICAL_CONTEXT",
    // --- units ---
    NamedUnit = "NAMED_UNIT",
    SiUnit = "SI_UNIT",
    LengthUnit = "LENGTH_UNIT",
    PlaneAngleUnit = "PLANE_ANGLE_UNIT",
    SolidAngleUnit = "SOLID_ANGLE_UNIT",
    MassUnit = "MASS_UNIT",
    ConversionBasedUnit = "CONVERSION_BASED_UNIT",
    MeasureWithUnit = "MEASURE_WITH_UNIT",
    LengthMeasureWithUnit = "LENGTH_MEASURE_WITH_UNIT",
    PlaneAngleMeasureWithUnit = "PLANE_ANGLE_MEASURE_WITH_UNIT",
    UncertaintyMeasureWithUnit = "UNCERTAINTY_MEASURE_WITH_UNIT",
    DimensionalExponents = "DIMENSIONAL_EXPONENTS",
    DerivedUnit = "DERIVED_UNIT",
    DerivedUnitElement = "DERIVED_UNIT_ELEMENT",
    // --- presentation / styles ---
    StyledItem = "STYLED_ITEM",
    OverRidingStyledItem = "OVER_RIDING_STYLED_ITEM",
    PresentationStyleAssignment = "PRESENTATION_STYLE_ASSIGNMENT",
    PresentationStyleByContext = "PRESENTATION_STYLE_BY_CONTEXT",
    SurfaceStyleUsage = "SURFACE_STYLE_USAGE",
    SurfaceSideStyle = "SURFACE_SIDE_STYLE",
    SurfaceStyleFillArea = "SURFACE_STYLE_FILL_AREA",
    SurfaceStyleRendering = "SURFACE_STYLE_RENDERING",
    SurfaceStyleRenderingWithProperties = "SURFACE_STYLE_RENDERING_WITH_PROPERTIES",
    SurfaceStyleTransparent = "SURFACE_STYLE_TRANSPARENT",
    FillAreaStyle = "FILL_AREA_STYLE",
    FillAreaStyleColour = "FILL_AREA_STYLE_COLOUR",
    ColourRgb = "COLOUR_RGB",
    Colour = "COLOUR",
    DraughtingPreDefinedColour = "DRAUGHTING_PRE_DEFINED_COLOUR",
    PreDefinedColour = "PRE_DEFINED_COLOUR",
    CurveStyle = "CURVE_STYLE",
    DraughtingPreDefinedCurveFont = "DRAUGHTING_PRE_DEFINED_CURVE_FONT",
    PointStyle = "POINT_STYLE",
    MechanicalDesignGeometricPresentationRepresentation = "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION",
    PresentationLayerAssignment = "PRESENTATION_LAYER_ASSIGNMENT",
    Invisibility = "INVISIBILITY",
    DraughtingModel = "DRAUGHTING_MODEL",
}

impl EntityType {
    /// Is this a curve entity (used to skip curve-targeting style items quickly)?
    pub fn is_curve(self) -> bool {
        use EntityType::*;
        matches!(
            self,
            Line | Circle
                | Ellipse
                | Hyperbola
                | Parabola
                | Polyline
                | BSplineCurve
                | BSplineCurveWithKnots
                | BezierCurve
                | QuasiUniformCurve
                | UniformCurve
                | RationalBSplineCurve
                | BoundedCurve
                | TrimmedCurve
                | CompositeCurve
                | Pcurve
                | SurfaceCurve
                | SeamCurve
                | IntersectionCurve
                | Curve
        )
    }

    /// Is this a shape representation carrying geometry items?
    pub fn is_shape_representation(self) -> bool {
        use EntityType::*;
        matches!(
            self,
            ShapeRepresentation
                | AdvancedBrepShapeRepresentation
                | ManifoldSurfaceShapeRepresentation
                | FacetedBrepShapeRepresentation
                | GeometricallyBoundedSurfaceShapeRepresentation
                | GeometricallyBoundedWireframeShapeRepresentation
                | EdgeBasedWireframeShapeRepresentation
        )
    }
}
