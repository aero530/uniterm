use ansi_parser::{Output, AnsiParser};
use ansi_parser::AnsiSequence;
use tracing::{debug, error};
use serde::{Deserialize, Serialize};


#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ActiveStyles<'a> {
	pub font_bold: bool,
	pub font_light: bool,
	pub italic: bool,
	pub underline: bool,
	pub blink: bool,
	pub reversed: bool,
	pub hidden: bool,
	pub line_through: bool,
	pub font_normal: bool,
	pub text: &'a str,
	pub bg: &'a str,
}

impl ActiveStyles<'_> {
	pub fn clear(&mut self) {
		self.font_bold = false;
		self.font_light = false;
		self.italic = false;
		self.underline = false;
		self.blink = false;
		self.reversed = false;
		self.hidden = false;
		self.line_through = false;
		self.font_normal = false;
		self.text = "";
		self.bg = "";
	}

	pub fn print(&self) -> String {
		let mut styles = Vec::new();
		if self.font_bold {styles.push("font-weight:bold;")};
		if self.font_light {styles.push("font-weight:light;")};
		if self.italic {styles.push("font-style:italic;")};
		if self.underline {styles.push("text-decoration:underline;")};
		if self.text.len()>0 {styles.push(self.text)};
		if self.bg.len()>0 {styles.push(self.bg)};
		styles.join(" ")
	}
}

pub fn ansi_to_html(bytes: &[u8]) -> String {

	let input = String::from_utf8_lossy(bytes).into_owned();

	let ansi_vec: Vec<Output> = input.ansi_parse().collect();
	let mut active_styles = ActiveStyles::default();
	let mut output = Vec::new();

	ansi_vec.iter().for_each(|ansi_seq| {
		let mut text = "";

		match ansi_seq {
			Output::TextBlock(s) => {
				text = s;
			},
			Output::Escape(seq) => {
				match seq {
					AnsiSequence::SetGraphicsMode(d) => {
						d.iter().for_each(|s| {
							graphics_mode_class(s, &mut active_styles)
						});
					},
					AnsiSequence::EraseDisplay => {
						output.clear();
					},
					AnsiSequence::CursorPos(_x,_y) => {
						// debug!("Go to {x}, {y}");
					},
					e => {debug!("{:?}",e);}
				}
			}
		}

		if text.len() > 0 {
			// Replacing line breaks with this div requires the container to be "flex" & "flex-wrap"
			let eols = text.matches("\r\n").map(|_| "<div style=\"flex-basis:100%;height:0;\"></div>").collect::<Vec<&str>>().join("");

			output.push(format!("<div style=\"{}\">{text}</div>{}", active_styles.print(), eols));
		}

	});

	output.join("")
}


/// Map ANSI Select Graphic Rendition parameters to css styles 
/// 
/// https://en.wikipedia.org/wiki/ANSI_escape_code#SGR_(Select_Graphic_Rendition)_parameters
fn graphics_mode_class(seq: &u8, styles: &mut ActiveStyles) {
	match seq {
		0 => styles.clear(),
		1 => styles.font_bold = true,
		2 => styles.font_light = true,
		3 => styles.italic = true,
		4 => styles.underline = true,
		5 => styles.blink = true,
		7 => styles.reversed = true,
		8 => styles.hidden = true,
		9 => styles.line_through = true,
		22 => styles.font_normal = true,
		23 => styles.italic = false,
		24 => styles.underline = false,
		25 => styles.blink = false,
		27 => styles.reversed = false,
		28 => styles.hidden = false,
		29 => styles.line_through = false,
		30 => styles.text = "color:rgb(  1,  1,  1);",	// black
		31 => styles.text = "color:rgb(222, 56, 43);",	// red
		32 => styles.text = "color:rgb( 57,181, 74);",	// green
		33 => styles.text = "color:rgb(255,199,  6);",	// yellow
		34 => styles.text = "color:rgb(  0,111,184);",	// blue
		35 => styles.text = "color:rgb(118, 38,113);",	// magenta
		36 => styles.text = "color:rgb( 44,181,233);",	// cyan
		37 => styles.text = "color:rgb(204,204,204);",	// white
		40 => styles.bg = "background-color:rgb(  1,  1,  1);",	// black
		41 => styles.bg = "background-color:rgb(222, 56, 43);",	// red
		42 => styles.bg = "background-color:rgb( 57,181, 74);",	// green
		43 => styles.bg = "background-color:rgb(255,199,  6);",	// yellow
		44 => styles.bg = "background-color:rgb(  0,111,184);",	// blue
		45 => styles.bg = "background-color:rgb(118, 38,113);",	// magenta
		46 => styles.bg = "background-color:rgb( 44,181,233);",	// cyan
		47 => styles.bg = "background-color:rgb(204,204,204);",	// white
		90 => styles.text = "color:rgb(128,128,128);",	// bright black
		91 => styles.text = "color:rgb(255,  0,  0);",	// bright red
		92 => styles.text = "color:rgb(  0,255,  0);",	// bright green
		93 => styles.text = "color:rgb(  0,255,  0);",	// bright yellow
		94 => styles.text = "color:rgb(  0,  0,255);",	// bright blue
		95 => styles.text = "color:rgb(255,  0,255);",	// bright magenta
		96 => styles.text = "color:rgb(  0,255,255);",	// bright cyan
		97 => styles.text = "color:rgb(255,255,255);",	// bright white
		100 => styles.bg = "background-color:rgb(128,128,128);",	// bright black
		101 => styles.bg = "background-color:rgb(255,  0,  0);",	// bright red
		102 => styles.bg = "background-color:rgb(  0,255,  0);",	// bright green
		103 => styles.bg = "background-color:rgb(  0,255,  0);",	// bright yellow
		104 => styles.bg = "background-color:rgb(  0,  0,255);",	// bright blue
		105 => styles.bg = "background-color:rgb(255,  0,255);",	// bright magenta
		106 => styles.bg = "background-color:rgb(  0,255,255);",	// bright cyan
		107 => styles.bg = "background-color:rgb(255,255,255);",	// bright white
		_ => {},
	};
}

// fn extended_ansii(char: &u8) -> &str {
// 	match char {
// 	128	=> "€",
// 	129	=> "",
// 	130	=> "‚",
// 	131	=> "ƒ",
// 	132	=> "„",
// 	133	=> "…",
// 	134	=> "†",
// 	135	=> "‡",
// 	136	=> "ˆ",
// 	137	=> "‰",
// 	138	=> "Š",
// 	139	=> "‹",
// 	140	=> "Œ",
// 	141	=> " ",
// 	142	=> "Ž",
// 	143	=> "",
// 	144	=> "",
// 	145	=> "‘",
// 	146	=> "’",
// 	147	=> "“",
// 	148	=> "”",
// 	149	=> "•",
// 	150	=> "–",
// 	151	=> "—",
// 	152	=> "˜",
// 	153	=> "™",
// 	154	=> "š",
// 	155	=> "›",
// 	156	=> "œ",
// 	157	=> "",
// 	158	=> "ž",
// 	159	=> "Ÿ",
//   	160	=> " ",
// 	161	=> "¡",
// 	162	=> "¢",
// 	163	=> "£",
// 	164	=> "¤",
// 	165	=> "¥",
// 	166	=> "¦",
// 	167	=> "§",
// 	168	=> "¨",
// 	169	=> "©",
// 	170	=> "ª",
// 	171	=> "«",
// 	172	=> "¬",
// 	173	=> "­",
// 	174	=> "®",
// 	175	=> "¯",
// 	176	=> "°",
// 	177	=> "±",
// 	178	=> "²",
// 	179	=> "³",
// 	180	=> "´",
// 	181	=> "µ",
// 	182	=> "¶",
// 	183	=> "·",
// 	184	=> "¸",
// 	185	=> "¹",
// 	186	=> "º",
// 	187	=> "»",
// 	188	=> "¼",
// 	189	=> "½",
// 	190	=> "¾",
// 	191	=> "¿",
// 	192	=> "À",
// 	193	=> "Á",
// 	194	=> "Â",
// 	195	=> "Ã",
// 	196	=> "Ä",
// 	197	=> "Å",
// 	198	=> "Æ",
// 	199	=> "Ç",
// 	200	=> "È",
// 	201	=> "É",
// 	202	=> "Ê",
// 	203	=> "Ë",
// 	204	=> "Ì",
// 	205	=> "Í",
// 	206	=> "Î",
// 	207	=> "Ï",
// 	208	=> "Ð",
// 	209	=> "Ñ",
// 	210	=> "Ò",
// 	211	=> "Ó",
// 	212	=> "Ô",
// 	213	=> "Õ",
// 	214	=> "Ö",
// 	215	=> "×",
// 	216	=> "Ø",
// 	217	=> "Ù",
// 	218	=> "Ú",
// 	219	=> "Û",
// 	220	=> "Ü",
// 	221	=> "Ý",
// 	222	=> "Þ",
// 	223	=> "ß",
// 	224	=> "à",
// 	225	=> "á",
// 	226	=> "â",
// 	227	=> "ã",
// 	228	=> "ä",
// 	229	=> "å",
// 	230	=> "æ",
// 	231	=> "ç",
// 	232	=> "è",
// 	233	=> "é",
// 	234	=> "ê",
// 	235	=> "ë",
// 	236	=> "ì",
// 	237	=> "í",
// 	238	=> "î",
// 	239	=> "ï",
// 	240	=> "ð",
// 	241	=> "ñ",
// 	242	=> "ò",
// 	243	=> "ó",
// 	244	=> "ô",
// 	245	=> "õ",
// 	246	=> "ö",
// 	247	=> "÷",
// 	248	=> "ø",
// 	249	=> "ù",
// 	250	=> "ú",
// 	251	=> "û",
// 	252	=> "ü",
// 	253	=> "ý",
// 	254	=> "þ",
// 	255	=> "ÿ",
// 	_ => "",
// 	}
// }