{
  pkgs ? import (builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/e513db7b0a65def6f6e17bb55195dfe510871944.tar.gz";
    sha256 = "sha256-RkAXQ3+a5rHMiP97Er7AawR1kvEXX9tK1mhiWedOPTc=";
  }) { },
}:
let
  runtimeLibraries = [
    pkgs.vulkan-loader
    pkgs.wayland
    pkgs.libxkbcommon
    pkgs.dbus
  ];
in
pkgs.mkShell {
  packages = [
    pkgs.directx-shader-compiler
    pkgs.spirv-tools
    pkgs.vulkan-loader
    pkgs.vulkan-validation-layers
    pkgs.vulkan-tools
    pkgs.mesa
    pkgs.wayland
    pkgs.wayland-protocols
    pkgs.wayland-utils
    pkgs.libxkbcommon
    pkgs.xkeyboard_config
    pkgs.adwaita-icon-theme
    pkgs.sway
    pkgs.grim
    pkgs.dbus
    pkgs.at-spi2-core
    pkgs.pkg-config
  ];
  AQUA_DXC_SPIRV = "${pkgs.directx-shader-compiler}/bin/dxc";
  AQUA_ATSPI_BUS_LAUNCHER = "${pkgs.at-spi2-core}/libexec/at-spi-bus-launcher";
  VK_ICD_FILENAMES = "${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.x86_64.json";
  VK_LAYER_PATH = "${pkgs.vulkan-validation-layers}/share/vulkan/explicit_layer.d";
  XKB_CONFIG_ROOT = "${pkgs.xkeyboard_config}/share/X11/xkb";
  XCURSOR_PATH = "${pkgs.adwaita-icon-theme}/share/icons";
  XCURSOR_THEME = "Adwaita";
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibraries;
}
