//! The spellcheck dictionaries the app can download: Chromium's Hunspell
//! dictionaries (the same word lists Chrome and Discord use), pinned to one
//! revision of chromium/deps/hunspell_dictionaries and to the SHA-256 of each
//! file as downloaded. A file that does not match is refused.
//!
//! Left out: ta_IN (spellbook cannot parse its affix file); hu-HU (its .aff
//! says UTF-8 but uses raw single bytes as affix flags, which a UTF-8 reader
//! cannot keep apart); and xx_XX (a test file). To update: pick a new revision, download every .aff and .dic
//! at it, and recompute these hashes and sizes.

/// The repository revision every download comes from.
pub const REVISION: &str = "00f6e69c905b30fdf701cdcef6a517582d7e0028";

/// One dictionary.
pub struct Entry {
    /// Its file name stem in the repository, e.g. `en_US`.
    pub code: &'static str,
    /// What to call it.
    pub name: &'static str,
    /// SHA-256 of the .aff file, as served (after base64 decoding).
    pub aff: &'static str,
    /// SHA-256 of the .dic file.
    pub dic: &'static str,
    /// Both files' size together, for the download prompt.
    pub bytes: u64,
}

/// Where a file of the pinned revision is served, base64-encoded.
pub fn url(code: &str, ext: &str) -> String {
    format!("https://chromium.googlesource.com/chromium/deps/hunspell_dictionaries/+/{REVISION}/{code}.{ext}?format=TEXT")
}

pub fn find(code: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.code == code)
}

pub static CATALOG: &[Entry] = &[
    Entry {
        code: "bg_BG",
        name: "Bulgarian",
        aff: "9bea9e139f54ffe70aece9380efe641e37bae6702514578e03305858bf0e50f6",
        dic: "c1d403ecf6588c3e23f9b57db2f905fda035e17c115d05736e1ce02ea17dacee",
        bytes: 1009682,
    },
    Entry {
        code: "ca_ES",
        name: "Catalan",
        aff: "763d019671be5f6c19baae3648a90d415a9f7c88e3cf32e2a9cb502914483bad",
        dic: "c8d7cc5483b35407a0ed5982e98f520c94cceb04eaae1a5ae25be3edbfcd4a9c",
        bytes: 2027671,
    },
    Entry {
        code: "cs_CZ",
        name: "Czech",
        aff: "cae05f86d965df73f192c2136ca3084e5cda2e7c1ef77224a0c6bede0a7f2118",
        dic: "5f90c3fa85a5e758752d7204eea3f44d5617eff7121c396aba5c86087343bcd5",
        bytes: 3861328,
    },
    Entry {
        code: "cy_GB",
        name: "Welsh",
        aff: "a348d6731d3c13afd922fbdf85a5194f9b818eb38568ccceba0c0540772b5b2a",
        dic: "fc710c8073ce8caf7215717aeb74f9a4c0eb32ed055b5cc9a0911f13145ff1ed",
        bytes: 705870,
    },
    Entry {
        code: "da_DK",
        name: "Danish",
        aff: "266f520af56e87d8825ac5ed0e73b823a0cf32f728ded0125332d46bfaa6df34",
        dic: "3b092c60daf950ff0672564ed6015a102241a2431bf5ae0b5588a71ff7b569bc",
        bytes: 1279885,
    },
    Entry {
        code: "de_DE_neu",
        name: "German",
        aff: "18734086dd466fdd45cc70bebc1503b923fae107e75c5fc14b929809fe855486",
        dic: "12776d068898107ec5bf60db6e75f5112c58bdf668de9d99440b2441768716ce",
        bytes: 10079533,
    },
    Entry {
        code: "el_GR",
        name: "Greek",
        aff: "62dcd6460058f208c278025df61021cace57b69b58271577095531e86f0b7433",
        dic: "552fe119d0e3e50ec23e7489debffde651a55732b1521f61247b684148fefb14",
        bytes: 6869603,
    },
    Entry {
        code: "en_AU",
        name: "English (Australia)",
        aff: "c7a8c4d08c29d237880844b1623099f59092602f189be38ce3912e457ff38bc1",
        dic: "aa07c46571f306b79fc1bc534357ed357af15687381b26f891ba66e8a2caed89",
        bytes: 557426,
    },
    Entry {
        code: "en_CA",
        name: "English (Canada)",
        aff: "c7a8c4d08c29d237880844b1623099f59092602f189be38ce3912e457ff38bc1",
        dic: "53096689870246a78fa408b1e7a8c49bc140bd466a84a384cfb509445fff7173",
        bytes: 554673,
    },
    Entry {
        code: "en_GB",
        name: "English (UK)",
        aff: "c7a8c4d08c29d237880844b1623099f59092602f189be38ce3912e457ff38bc1",
        dic: "869fe17ba4ee4b5401c60a666ee2d6a3dcc237f460b7294435df1cc6a799aa57",
        bytes: 555196,
    },
    Entry {
        code: "en_GB_oxendict",
        name: "English (UK, Oxford)",
        aff: "c7a8c4d08c29d237880844b1623099f59092602f189be38ce3912e457ff38bc1",
        dic: "d5545f97b5bd180cf16d30015093eb5a2a657a2d4502763e5e8173dd4b1d86ac",
        bytes: 554882,
    },
    Entry {
        code: "en_US",
        name: "English (US)",
        aff: "c7a8c4d08c29d237880844b1623099f59092602f189be38ce3912e457ff38bc1",
        dic: "f0b1a234bd178bdd01875b2a392a9647f888b8fe879f79c52aae62c2759b3647",
        bytes: 554852,
    },
    Entry {
        code: "es_ES",
        name: "Spanish",
        aff: "cd48717bd8a913e77164b65ebc9492b7debbd802ec717dfa6bc2f0ddcf4c4a86",
        dic: "c6a62679e4551eac6374707f4e2ea78d708ee9707d1f6f743fb1d6adb497e18d",
        bytes: 997452,
    },
    Entry {
        code: "et_EE",
        name: "Estonian",
        aff: "2fa53b7853762ec8c63ffa14b2d96d23433e3f2c2d3d218ad7c2461673099f95",
        dic: "cd1378434aefeaa8a31f49369dbf71caf4e6340badb5c2cf7a55820933ed4f13",
        bytes: 4620177,
    },
    Entry {
        code: "fa_IR",
        name: "Persian",
        aff: "dba0980816354ad969aa46dda0c040eeebda4a9381be331e17eae8647a2a1e1c",
        dic: "314f31e059994f26e8016c834b14dfd10f74834b3f623a6040265b8f2a9dddab",
        bytes: 2581430,
    },
    Entry {
        code: "fr_FR",
        name: "French",
        aff: "f14b51eba2e1ae14236d95d4497342bf9224e9d0a19d04f1e02615f08288b7f8",
        dic: "390db352e3cb88a33f0a2e4e4878032186f2aec9889f9267b4b5ef91c7544d3f",
        bytes: 1402613,
    },
    Entry {
        code: "gl",
        name: "Galician",
        aff: "509da3926c02d5b225382e6ef39edf531c1659e4ab841c8076ebc08a9e85b2b9",
        dic: "81a2c83bcb9979ecca2b25126ed58c6b2836988cce328e52223c2c2ba0967ef0",
        bytes: 2802708,
    },
    Entry {
        code: "he_IL",
        name: "Hebrew",
        aff: "92bdca42e62f64f708f743df70f155012a6cc8e0a2704f8c007c99b052426c43",
        dic: "96f47dc1aae45416e9638a7338f4a68f7f92e12e1d44b4baa4c4efc73ec69eaf",
        bytes: 3278305,
    },
    Entry {
        code: "hi_IN",
        name: "Hindi",
        aff: "15afdbd926b16bc52344a6f94d965c84f5697839408ce68a5196f19ee606caf9",
        dic: "c958997a2e3e30fe36589f4e79defd2411c1b6025db4341524e393810b55b607",
        bytes: 320220,
    },
    Entry {
        code: "hr_HR",
        name: "Croatian",
        aff: "d88ac0ab158304f9e921ba9209830be966f1a8122eee0bee437096176550e5c5",
        dic: "d66a17d1cf72c6fc994147221e81a7b504e3b1a1a9827c656b133a00fe36da61",
        bytes: 2354612,
    },
    Entry {
        code: "hy",
        name: "Armenian",
        aff: "71ec439a379468015be67253a7f1bec82e8824be439de571a7ff30c69eee3795",
        dic: "0fce278313fa6ecedec8d4a32800a9102925bd2e1c794fdc2012f31721805547",
        bytes: 22357517,
    },
    Entry {
        code: "id_ID",
        name: "Indonesian",
        aff: "9c9f237ac8c0838b2ce71a8d04db23d4cab09c0beafbf3070716df796ae2d2f4",
        dic: "d97001789c6bc4856d8f2af3b8450bbcc43320c4fddf0af8af84fa97dcfb5ac2",
        bytes: 223057,
    },
    Entry {
        code: "it_IT",
        name: "Italian",
        aff: "3b949c06c73d29432f17777986d901b21ab03556748b1bec03c42ada9f67e2c9",
        dic: "02e16bee63777739d404755276d04c4949435cebdcae62f152bba605230de3d5",
        bytes: 1469795,
    },
    Entry {
        code: "ko",
        name: "Korean",
        aff: "0dd4991593f8fcf34ceb679a9cbd0774e46a565029e3bfbdfee37706a564c817",
        dic: "9813b8a64568a9a884d9b4c4e8121045be7b49faad1340bfee7fb2b411206c3e",
        bytes: 16932549,
    },
    Entry {
        code: "lt_LT",
        name: "Lithuanian",
        aff: "dd12b37ddfa7fcb78ed623ac5da4f4a1236f91cc11e9df94116e056eae9e7544",
        dic: "f56fae91ff777fbcaa8ca74a9a82ef892fe733916e5078e1daa52e7b7b47c900",
        bytes: 1212734,
    },
    Entry {
        code: "lv_LV",
        name: "Latvian",
        aff: "235610eb228c3ce9879814a141df5ea73d9b493c639157749c1b3a481eaf6c7c",
        dic: "79a10fe6f807309a7e0aa0ee38d3343f3a09110e2b4ca9c62ef79ddde7d6f54e",
        bytes: 2296431,
    },
    Entry {
        code: "nb_NO",
        name: "Norwegian (Bokmål)",
        aff: "b9e828538222b3580d8adb4660d5bae9bc1fbe42e7c21fd3e01970efed96b4f8",
        dic: "aadaf9ea6b437b4edada54a2084e1c8dc00029addc6dfb4e15637f8e798dacc1",
        bytes: 9353647,
    },
    Entry {
        code: "nl_NL",
        name: "Dutch",
        aff: "9a06227b61fed41c0eed4eee1cc4376733572ef4c7a6a9c8886cc38d6b2046fb",
        dic: "7d8bddf6e7dbe4a36cdc9f7d87562adcfa4a6981487d84950758f81f6a61b0cf",
        bytes: 2449456,
    },
    Entry {
        code: "nn_NO",
        name: "Norwegian (Nynorsk)",
        aff: "454f0bf09e6581ca8e153c245a6136bec15e954f752d930ad32fedeb012ce5de",
        dic: "9173f6b9e943cea8e3a2097b7c1cd67e09a0839cb85dfaad9114d7fc2c313c81",
        bytes: 6481290,
    },
    Entry {
        code: "pl_PL",
        name: "Polish",
        aff: "c587147572c9c5e20a6a424bf4b4f79e55a2603119f26130acdad20cc0487fa2",
        dic: "d227ab4013ebfe9a02e465379e7dfb1216d2496fc2148b49dcf7b3dd4190dbb0",
        bytes: 4766604,
    },
    Entry {
        code: "pt_BR",
        name: "Portuguese (Brazil)",
        aff: "8427837d0ce374c628e8e90fbffab55f617f345d1774e9f9d1e2bef489b0508a",
        dic: "bc9410b115fb717773585658dc14c0d98d0933db87ae175c160c2321f4a9c273",
        bytes: 5552273,
    },
    Entry {
        code: "pt_PT",
        name: "Portuguese (Portugal)",
        aff: "505ef44f2af3d3c18eb97496509c3c370a75ea76000a7d3d40d68d626016af5e",
        dic: "7cc363daf25ffeb45b62632fa448b89d7ce90c62e7d24ec24625ccb276da7509",
        bytes: 488520,
    },
    Entry {
        code: "ro_RO",
        name: "Romanian",
        aff: "6bb1361090f76785e5da44d1dca50d911ad5b1e94b027379ff62e19aaa5ba55e",
        dic: "c38361d0bce03567dead9c2b08fd75a523e3fa4ad86e3964a6d40cf66a6118ab",
        bytes: 1967223,
    },
    Entry {
        code: "ru_RU",
        name: "Russian",
        aff: "f868ff4913af498d342fd9423d5a5295720aa2d66d0d9e7a05fc18fa0eb5e236",
        dic: "12b781ad1c8305cde4cb737a78d1aafa755fe16e2615c4584c4b14b86e2f02cd",
        bytes: 2021849,
    },
    Entry {
        code: "sh",
        name: "Serbo-Croatian",
        aff: "6d19828d6838976956d81ccef349e3bcc08fe22edb3b5de8182143cd1dafa8ce",
        dic: "c8c889499b1827325fc29d08e10bb459b12537c559e1f9b430bca14c7faffe35",
        bytes: 4208201,
    },
    Entry {
        code: "sk_SK",
        name: "Slovak",
        aff: "4c0200d9674c0d46abee473c61951fa40a3af36089d0bfbc975925eeb590d0ff",
        dic: "260b62fd8b7c981d04063dbf4c79c927c50633b35bf7cde29a9c4da6841f4112",
        bytes: 2024365,
    },
    Entry {
        code: "sl_SI",
        name: "Slovenian",
        aff: "705dfac070528217af12528f0b9af6dd0edef2a7f7b4d29eee29e7c7c002a895",
        dic: "11cbd542549357e6fe8048ed6b9b3940351b0c15626c01f47cf05f01e302b3f7",
        bytes: 3230008,
    },
    Entry {
        code: "sq",
        name: "Albanian",
        aff: "fbb5a2a0a1a4bcd1d53b4b4b350b3b0cef8c92f68f51813be59f4e3a33be1d8d",
        dic: "1f9fa0dae2d7d25158fad2e1f9a2098d8bf770e7935381d11a32fa1cb21608ec",
        bytes: 2612733,
    },
    Entry {
        code: "sr",
        name: "Serbian",
        aff: "dc7c6e381e628536b81afd46bfac9af77da51f1c270cbc09bcee911d996fa41e",
        dic: "db87d8626f82cf69416686f67dea7556fffbcc683a957730268817df67ad4bd0",
        bytes: 6493463,
    },
    Entry {
        code: "sv_SE",
        name: "Swedish",
        aff: "3419fdfbd5ce4e357972c8eb11778a9a7303021ce872a449acdcef76e1b8d7b4",
        dic: "91dfad16ffb251d8bdd57a0a7487511b399676f51e11fdc18716a57e6e7506c2",
        bytes: 2093941,
    },
    Entry {
        code: "tg_TG",
        name: "Tajik",
        aff: "385a26f32f1959ac161f42cf9aaa929a8bac5b4e706283b2077dbb353b0e003d",
        dic: "09928d98af12676919fac1bb6d41120dd5117cd8caf8a6ee89feb25b979c25eb",
        bytes: 1706882,
    },
    Entry {
        code: "tr",
        name: "Turkish",
        aff: "f1b4cdcf8f95cd86b315a7f57e87514d39008d76b187cfeb13b58065ba6b3f43",
        dic: "02bd6d708fb745b8d85407f6840c9f091bcaa76ccd35041752d2a95b18b2c0d8",
        bytes: 11178206,
    },
    Entry {
        code: "uk_UA",
        name: "Ukrainian",
        aff: "8a06fdceab79087764948e29c06d1675c0a59a5301271bc34d85bae7960e8585",
        dic: "721a58bdcb1d19d5dc4e4a0aef05ad9604f209f6b00483956143e234b090e783",
        bytes: 8555431,
    },
    Entry {
        code: "vi_VN",
        name: "Vietnamese",
        aff: "32fcca73da23005c458f087ff090b4c6bb942215def1461257685d92ff84e6a3",
        dic: "10643c4a14126158ad4c40a05797e08106181d6573167f892f7f90f6645f674c",
        bytes: 48270,
    },
];
