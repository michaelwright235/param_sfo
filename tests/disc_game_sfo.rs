use param_sfo::{ParamSFO, param_sfo};

#[test]
fn disc_game_sfo() {
    let original_bytes = std::fs::read("./tests/PARAM.SFO").unwrap();
    let original = ParamSFO::from_bytes(&original_bytes).unwrap();

    // test that the macro works fine with vars
    let key = "APP_VER";
    let val = "01.00";

    let custom = param_sfo! {
        "SOUND_FORMAT" => [263, 4], // test the automatic alphabetical order
        key => [val, 8],
        "ATTRIBUTE" => [165, 4],
        "BOOTABLE" => [1, 4],
        "CATEGORY" => ["DG", 4],
        "LICENSE" => ["Library programs ©Sony Computer Entertainment Inc. Licensed for play on the PLAYSTATION®3 Computer Entertainment System or authorized PLAYSTATION®3 format systems. For full terms and conditions see the user's manual. This product is authorized and produced under license from Sony Computer Entertainment Inc. Use is subject to the copyright laws and the terms and conditions of the user's license.", 512],
        "PARENTAL_LEVEL" => [9, 4],
        "PS3_SYSTEM_VER" => ["04.1100", 8],
        "RESOLUTION" => [55, 4],
        "TITLE" => ["LOLLIPOP CHAINSAW", 128],
        "TITLE_ID" => ["BLES01525", 16],
        "VERSION" => ["01.01", 8]
    }.unwrap();

    assert_eq!(original, custom);
    assert_eq!(original_bytes, custom.to_bytes().unwrap());
}
