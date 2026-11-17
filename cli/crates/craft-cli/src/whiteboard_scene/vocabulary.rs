//! Named values for the whiteboard editor's numeric and RGB encodings.
//!
//! Names are the scene vocabulary; codes are what the editor stores. The editor renders a color
//! that is not in its token palette as black, so scenes may only name palette colors.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! coded {
    ($(#[$meta:meta])* $name:ident: $code:ty { $($variant:ident = $text:literal => $value:expr,)+ }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant,)+
        }
        impl $name {
            pub const ALL: &[$name] = &[$($name::$variant,)+];
            pub fn code(self) -> $code {
                match self { $($name::$variant => $value,)+ }
            }
            pub fn name(self) -> &'static str {
                match self { $($name::$variant => $text,)+ }
            }
            pub fn from_code(code: $code) -> Option<Self> {
                Self::ALL.iter().copied().find(|item| item.code() == code)
            }
        }
    };
}

coded! {
    /// Palette color. `<hue>-subtlest`, `-subtle`, plain, `-bold` and `-boldest` follow the
    /// Atlassian 100, 200, 300, 600 and 800 accent tokens.
    Color: (u8, u8, u8) {
        White = "white" => (255, 255, 255),
        DefaultText = "default-text" => (23, 43, 77),
        GraySubtle = "gray-subtle" => (241, 242, 244),
        Gray = "gray" => (179, 185, 196),
        GrayBold = "gray-bold" => (117, 129, 149),
        GrayBoldest = "gray-boldest" => (68, 84, 111),
        BlueSubtlest = "blue-subtlest" => (233, 242, 255),
        BlueSubtle = "blue-subtle" => (204, 224, 255),
        Blue = "blue" => (133, 184, 255),
        BlueBold = "blue-bold" => (29, 122, 252),
        BlueBoldest = "blue-boldest" => (0, 85, 204),
        TealSubtlest = "teal-subtlest" => (227, 250, 252),
        TealSubtle = "teal-subtle" => (200, 244, 249),
        Teal = "teal" => (116, 219, 231),
        TealBold = "teal-bold" => (29, 154, 170),
        TealBoldest = "teal-boldest" => (32, 107, 116),
        GreenSubtlest = "green-subtlest" => (223, 252, 240),
        GreenSubtle = "green-subtle" => (199, 248, 227),
        Green = "green" => (118, 224, 180),
        GreenBold = "green-bold" => (34, 160, 107),
        GreenBoldest = "green-boldest" => (33, 110, 78),
        LimeSubtlest = "lime-subtlest" => (239, 255, 214),
        LimeSubtle = "lime-subtle" => (211, 241, 167),
        Lime = "lime" => (179, 233, 114),
        LimeBold = "lime-bold" => (106, 154, 35),
        LimeBoldest = "lime-boldest" => (76, 107, 31),
        YellowSubtlest = "yellow-subtlest" => (255, 247, 214),
        YellowSubtle = "yellow-subtle" => (255, 239, 174),
        Yellow = "yellow" => (255, 222, 92),
        YellowBold = "yellow-bold" => (179, 134, 0),
        YellowBoldest = "yellow-boldest" => (127, 95, 1),
        OrangeSubtlest = "orange-subtlest" => (255, 245, 235),
        OrangeSubtle = "orange-subtle" => (255, 222, 184),
        Orange = "orange" => (254, 193, 118),
        OrangeBold = "orange-bold" => (219, 110, 0),
        OrangeBoldest = "orange-boldest" => (151, 79, 12),
        RedSubtlest = "red-subtlest" => (255, 237, 235),
        RedSubtle = "red-subtle" => (255, 210, 204),
        Red = "red" => (255, 154, 143),
        RedBold = "red-bold" => (232, 66, 44),
        RedBoldest = "red-boldest" => (174, 42, 25),
        MagentaSubtlest = "magenta-subtlest" => (255, 236, 248),
        MagentaSubtle = "magenta-subtle" => (253, 208, 236),
        Magenta = "magenta" => (247, 151, 210),
        MagentaBold = "magenta-bold" => (205, 81, 157),
        MagentaBoldest = "magenta-boldest" => (148, 61, 115),
        PurpleSubtlest = "purple-subtlest" => (243, 240, 255),
        PurpleSubtle = "purple-subtle" => (223, 216, 253),
        Purple = "purple" => (184, 172, 246),
        PurpleBold = "purple-bold" => (130, 112, 219),
        PurpleBoldest = "purple-boldest" => (94, 77, 178),
    }
}

impl Color {
    /// The eight colors the sticky-note picker offers.
    pub fn is_sticky(self) -> bool {
        matches!(
            self,
            Color::YellowSubtle
                | Color::OrangeSubtle
                | Color::RedSubtle
                | Color::MagentaSubtle
                | Color::PurpleSubtle
                | Color::BlueSubtle
                | Color::TealSubtle
                | Color::GreenSubtle
        )
    }
}

coded! {
    /// Shape library entry. Class and Interface (codes 82 and 83) carry compartments that a
    /// scene cannot describe, so they are only preserved as raw elements.
    ShapeKind: u16 {
        SharpRectangle = "sharp_rectangle" => 0,
        Rectangle = "rectangle" => 1,
        Ellipse = "ellipse" => 2,
        RoundedRectangle = "rounded_rectangle" => 3,
        Diamond = "diamond" => 4,
        UpTriangle = "up_triangle" => 5,
        DownTriangle = "down_triangle" => 6,
        LeftParallelogram = "left_parallelogram" => 7,
        RightParallelogram = "right_parallelogram" => 8,
        StartEnd = "start_end" => 9,
        Document = "document" => 10,
        OffPage = "off_page" => 11,
        InputOutput = "input_output" => 12,
        Database = "database" => 13,
        SummingJunction = "summing_junction" => 14,
        Or = "or" => 15,
        PredefinedProcess = "predefined_process" => 16,
        InternalStorage = "internal_storage" => 17,
        ManualInput = "manual_input" => 18,
        ManualOperation = "manual_operation" => 19,
        MultipleDocuments = "multiple_documents" => 20,
        Preparation = "preparation" => 21,
        HardDisk = "hard_disk" => 22,
        CommentLeft = "comment_left" => 23,
        CommentRight = "comment_right" => 24,
        StoredData = "stored_data" => 25,
        Delay = "delay" => 26,
        Display = "display" => 27,
        Process = "process" => 28,
        Decision = "decision" => 29,
        Connector = "connector" => 30,
        Merge = "merge" => 31,
        Cloud = "cloud" => 32,
        Key = "key" => 33,
        Server = "server" => 34,
        Archive = "archive" => 35,
        Browser = "browser" => 36,
        User = "user" => 37,
        Compute = "compute" => 38,
        Computer = "computer" => 39,
        File = "file" => 40,
        Firewall = "firewall" => 41,
        Folder = "folder" => 42,
        Frontend = "frontend" => 43,
        Internet = "internet" => 44,
        Lock = "lock" => 45,
        Mail = "mail" => 46,
        Mobile = "mobile" => 47,
        Settings = "settings" => 48,
        Shield = "shield" => 49,
        Users = "users" => 50,
        Switch = "switch" => 51,
        DatabaseAdvanced = "database_advanced" => 52,
        AlternateProcess = "alternate_process" => 53,
        UseCase = "use_case" => 54,
        Classifier = "classifier" => 55,
        Note = "note" => 56,
        Interface2 = "interface2" => 57,
        Activation = "activation" => 58,
        Activity = "activity" => 59,
        Actor = "actor" => 60,
        Assembly = "assembly" => 61,
        Component = "component" => 62,
        Deletion = "deletion" => 63,
        End = "end" => 64,
        FlowFinal = "flow_final" => 65,
        Gateway = "gateway" => 66,
        HistoryPseudostate = "history_pseudostate" => 67,
        HorizontalFork = "horizontal_fork" => 68,
        VerticalFork = "vertical_fork" => 69,
        OffPageLink = "off_page_link" => 70,
        PinFilledLeft = "pin_filled_left" => 71,
        PinFilledRight = "pin_filled_right" => 72,
        PinLeft = "pin_left" => 73,
        PinRight = "pin_right" => 74,
        Pin = "pin" => 75,
        ProvidedInterface = "provided_interface" => 76,
        ReceiveSignal = "receive_signal" => 77,
        RequiredInterface = "required_interface" => 78,
        SendSignal = "send_signal" => 79,
        Start = "start" => 80,
        Template = "template" => 81,
        Node = "node" => 84,
        Container = "container" => 85,
        BoundaryObject = "boundary_object" => 86,
        EntityObject = "entity_object" => 87,
        ControlObject = "control_object" => 88,
    }
}

coded! {
    /// Line end marker.
    Cap: u8 {
        None = "none" => 1,
        Arrow = "arrow" => 2,
        FilledArrow = "filled_arrow" => 3,
        OpenArrow = "open_arrow" => 4,
        FilledDiamond = "filled_diamond" => 5,
        OpenDiamond = "open_diamond" => 6,
        OpenCircle = "open_circle" => 7,
        Slash = "slash" => 8,
        TripleBar = "triple_bar" => 9,
        OpenCircleCross = "open_circle_cross" => 10,
        Cross = "cross" => 11,
        CrossCrowsFoot = "cross_crows_foot" => 12,
        CrowsFoot = "crows_foot" => 13,
        CircleCrowsFoot = "circle_crows_foot" => 14,
    }
}

coded! {
    /// Path routing: straight, right-angled ("dynamic" in the editor) or curved.
    Route: u8 {
        Straight = "straight" => 1,
        Elbow = "elbow" => 2,
        Curved = "curved" => 3,
    }
}

coded! {
    /// Line weight (the editor's small, medium and large).
    Stroke: u8 {
        Thin = "thin" => 1,
        Medium = "medium" => 2,
        Thick = "thick" => 3,
    }
}

coded! {
    Dash: u8 {
        Solid = "solid" => 1,
        Dashed = "dashed" => 2,
        Dotted = "dotted" => 3,
    }
}

coded! {
    VerticalAlign: u8 {
        Top = "top" => 0,
        Middle = "middle" => 1,
        Bottom = "bottom" => 2,
    }
}

coded! {
    Align: &'static str {
        Left = "left" => "left",
        Center = "center" => "center",
        Right = "right" => "right",
    }
}

coded! {
    /// Stamp sprite identifiers offered by the stamp picker.
    Sprite: &'static str {
        ThumbsUp = "thumbs-up" => "thumbs-up",
        ThumbsDown = "thumbs-down" => "thumbs-down",
        Thanks = "thanks" => "thanks",
        ClappingHands = "clapping-hands" => "clapping-hands",
        HeartEyes = "heart-eyes" => "heart-eyes",
        Smile = "smile" => "smile",
        Joy = "joy" => "joy",
        Crazy = "crazy" => "crazy",
        PartyHat = "party-hat" => "party-hat",
        Shocked = "shocked" => "shocked",
        Cool = "cool" => "cool",
        StarEyes = "star-eyes" => "star-eyes",
        OkFace = "ok-face" => "ok-face",
        Peace = "peace" => "peace",
        DownPoint = "down-point" => "down-point",
        UpPoint = "up-point" => "up-point",
        RightPoint = "right-point" => "right-point",
        LeftPoint = "left-point" => "left-point",
        Fire = "fire" => "fire",
        Hundred = "100" => "100",
        Spicy = "spicy" => "spicy",
        Party = "party" => "party",
        Heart = "heart" => "heart",
        QuestionMark = "question-mark" => "question-mark",
        GoldStar = "gold-star" => "gold-star",
        Eyes = "eyes" => "eyes",
        PlusOne = "plus-one" => "plus-one",
        Win = "win" => "win",
        Tool = "tool" => "tool",
        Hourglass = "hourglass" => "hourglass",
        MagnifyingGlass = "magnifying-glass" => "magnifying-glass",
        Megaphone = "megaphone" => "megaphone",
        Bell = "bell" => "bell",
        Candy = "candy" => "candy",
        Calendar = "calendar" => "calendar",
        Idea = "idea" => "idea",
        Artist = "artist" => "artist",
        Rocket = "rocket" => "rocket",
        Success = "success" => "success",
        Note = "note" => "note",
        Info = "info" => "info",
        Question = "question" => "question",
        Decision = "decision" => "decision",
        Error = "error" => "error",
        Warning = "warning" => "warning",
    }
}
