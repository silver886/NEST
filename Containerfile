FROM docker.io/library/rust:1-slim AS base

RUN set -eux; \
    apt-get update && apt-get install --no-install-recommends -y \
        clang llvm lld \
        jq imagemagick; \
    command -v clang-cl >/dev/null || ln -s clang /usr/bin/clang-cl; \
    command -v llvm-lib >/dev/null || ln -s llvm-ar /usr/bin/llvm-lib; \
    command -v lld-link >/dev/null; \
    update-alternatives --install /usr/bin/cc cc /usr/bin/clang 100; \
    update-alternatives --install /usr/bin/c++ c++ /usr/bin/clang++ 100; \
    apt-get remove -y --auto-remove; \
    rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-pc-windows-msvc; \
    cargo install xwin; \
    xwin --accept-license splat --output /xwin; \
    rm -rf .xwin-cache /usr/local/cargo/bin/xwin

ENV CC_x86_64_pc_windows_msvc=clang-cl \
    CXX_x86_64_pc_windows_msvc=clang-cl \
    AR_x86_64_pc_windows_msvc=llvm-lib \
    CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=lld-link \
    CFLAGS_x86_64_pc_windows_msvc="-Wno-unused-command-line-argument -fuse-ld=lld-link /imsvc/xwin/crt/include /imsvc/xwin/sdk/include/ucrt /imsvc/xwin/sdk/include/um /imsvc/xwin/sdk/include/shared" \
    CXXFLAGS_x86_64_pc_windows_msvc="-Wno-unused-command-line-argument -fuse-ld=lld-link /imsvc/xwin/crt/include /imsvc/xwin/sdk/include/ucrt /imsvc/xwin/sdk/include/um /imsvc/xwin/sdk/include/shared" \
    RUSTFLAGS="-Lnative=/xwin/crt/lib/x86_64 -Lnative=/xwin/sdk/lib/um/x86_64 -Lnative=/xwin/sdk/lib/ucrt/x86_64"

FROM base AS build

ARG CONFIG_JSON=config.json

WORKDIR /var/workdir

COPY ./ ./

RUN set -eux; \
    test -f "${CONFIG_JSON}"; \
    jq -r 'def cfg(path; default): getpath(path) as $v | if $v == null then default else $v end; \
    def app_urls: \
      (.app.url | to_entries | map(select(.value | type == "string"))); \
    [ \
      "pub const APP_TITLE: &str = \(cfg(["app", "title"]; "NEST") | @json);", \
      "pub const APP_VERSION: &str = \(cfg(["app", "version"]; "0.1.0") | @json);", \
      "pub const APP_IDENTIFIER: &str = \(cfg(["app", "identifier"]; "com.example") | @json);", \
      "pub const APP_REUSE_INSTANCE: bool = \(cfg(["app", "reuse_instance"]; false));", \
      "pub const APP_URLS: &[(&str, &str)] = &[\((app_urls) | map("(\(.key | @json), \(.value | @json))") | join(", "))];", \
      "pub const MAILTO_URL_TEMPLATE: &str = \(cfg(["webview", "mailto_url_template"]; "https://example.com/") | @json);", \
      "pub const WINDOW_WIDTH: f64 = \(cfg(["window", "width"]; 1280)) as f64;", \
      "pub const WINDOW_HEIGHT: f64 = \(cfg(["window", "height"]; 720)) as f64;", \
      "pub const WINDOW_RESIZABLE: bool = \(cfg(["window", "resizable"]; true));", \
      "pub const WINDOW_TITLE_BAR: bool = \(cfg(["window", "title_bar"]; true));", \
      "pub const WINDOW_FULLSCREEN: bool = \(cfg(["window", "fullscreen"]; false));", \
      "pub const WINDOW_MAXIMIZED: bool = \(cfg(["window", "maximized"]; false));", \
      "pub const WINDOW_ALWAYS_ON_TOP: bool = \(cfg(["window", "always_on_top"]; false));", \
      "pub const WINDOW_CLOSE_TO_TRAY: bool = \(cfg(["window", "close_to_tray"]; false));", \
      "pub const WINDOW_TRAY_MENU: bool = \(cfg(["window", "tray_menu"]; false));", \
      "pub const WEBVIEW_INCOGNITO: bool = \(cfg(["webview", "incognito"]; true));", \
      "pub const ENABLE_DRAG_DROP: bool = \(cfg(["enable_drag_drop"]; true));", \
      "pub const ALLOW_NEW_WINDOWS: bool = \(cfg(["allow_new_windows"]; true));", \
      "pub const INTERNAL_URL_PREFIXES: &[&str] = &[\((.internal_url.prefixes // []) | map(@json) | join(", "))];", \
      "pub const INTERNAL_URL_REGEXES: &[&str] = &[\((.internal_url.regexes // []) | map(@json) | join(", "))];", \
      "pub const WEBVIEW_ARGS: &str = \(cfg(["webview", "args"]; "") | @json);" \
    ] | join("\n")' \
      "${CONFIG_JSON}" > ./app/src/app_config.rs

RUN set -eux; \
    sed -i 's/\r$//' ./app/Cargo.toml ./app/Cargo.lock; \
    old_cargo_name="$(awk '$1 == "name" && $2 == "=" { gsub("\"", "", $3); print $3; exit }' ./app/Cargo.toml)"; \
    old_cargo_version="$(awk '$1 == "version" && $2 == "=" { gsub("\"", "", $3); print $3; exit }' ./app/Cargo.toml)"; \
    cargo_name="$(jq -r '(.app.identifier // "nest") | gsub("[^A-Za-z0-9_-]"; "-") | if length == 0 then "nest" else . end' "${CONFIG_JSON}")"; \
    cargo_version="$(jq -r '.app.version // "0.1.0"' "${CONFIG_JSON}")"; \
    cargo_description="$(jq -r '.app.title // "NEST"' "${CONFIG_JSON}")"; \
    cargo_name_toml="$(jq -rn --arg value "$cargo_name" '$value | @json')"; \
    cargo_version_toml="$(jq -rn --arg value "$cargo_version" '$value | @json')"; \
    cargo_description_toml="$(jq -rn --arg value "$cargo_description" '$value | @json')"; \
    awk \
      -v name="$cargo_name_toml" \
      -v version="$cargo_version_toml" \
      -v description="$cargo_description_toml" \
      ' \
        $0 == "[package]" { in_package = 1; print; next } \
        /^\[/ && $0 != "[package]" { in_package = 0 } \
        in_package && $1 == "name" && $2 == "=" { print "name = " name; next } \
        in_package && $1 == "version" && $2 == "=" { print "version = " version; next } \
        in_package && $1 == "description" && $2 == "=" { print "description = " description; next } \
        { print } \
      ' ./app/Cargo.toml > ./app/Cargo.toml.tmp; \
    mv ./app/Cargo.toml.tmp ./app/Cargo.toml; \
    awk \
      -v old_name="$old_cargo_name" \
      -v old_version="$old_cargo_version" \
      -v name="$cargo_name_toml" \
      -v version="$cargo_version_toml" \
      ' \
        function flush() { \
          if (count == 0) return; \
          matched = 0; \
          for (i = 1; i <= count; i++) { \
            if (lines[i] == "name = \"" old_name "\"") matched_name = 1; \
            if (lines[i] == "version = \"" old_version "\"") matched_version = 1; \
            if (lines[i] ~ /^source = /) has_source = 1; \
          } \
          matched = matched_name && matched_version && !has_source; \
          if (matched) replacement_count++; \
          for (i = 1; i <= count; i++) { \
            if (matched && lines[i] == "name = \"" old_name "\"") print "name = " name; \
            else if (matched && lines[i] == "version = \"" old_version "\"") print "version = " version; \
            else print lines[i]; \
          } \
          count = 0; matched_name = 0; matched_version = 0; has_source = 0; \
        } \
        $0 == "[[package]]" { flush() } \
        { lines[++count] = $0 } \
        END { flush(); if (replacement_count != 1) exit 42 } \
      ' ./app/Cargo.lock > ./app/Cargo.lock.tmp; \
    mv ./app/Cargo.lock.tmp ./app/Cargo.lock

RUN set -eux; \
    icon_src="./$(jq -r '.app.icon // "icon/icon.png"' "${CONFIG_JSON}")"; \
    test -f "$icon_src"; \
    convert "$icon_src" -define icon:auto-resize=256,64,48,32,16 ./app/favicon.ico; \
    { \
      printf 'pub const APP_ICON_WIDTH: u32 = 32;\n'; \
      printf 'pub const APP_ICON_HEIGHT: u32 = 32;\n'; \
      printf 'pub const APP_ICON_RGBA: &[u8] = &[\n'; \
      convert "$icon_src" -resize 32x32\! RGBA:- | od -An -tx1 -v | awk '{ for (i=1; i<=NF; i++) printf "0x%s,", $i; printf "\n" }'; \
      printf '];\n'; \
    } > ./app/src/app_icon.rs

RUN set -eux; \
    mkdir -p ./out/; \
    output="$(realpath ./out/$(jq -r '.app.identifier // "app"' "${CONFIG_JSON}").exe)"; \
    cargo_name="$(jq -r '(.app.identifier // "nest") | gsub("[^A-Za-z0-9_-]"; "-") | if length == 0 then "nest" else . end' "${CONFIG_JSON}")"; \
    cargo_features="$(jq -r '[if (.window.close_to_tray // false) then "close-to-tray" else empty end, if (.app.reuse_instance // false) then "reuse-instance" else empty end] | if length > 0 then "--features=" + join(",") else "" end' "${CONFIG_JSON}")"; \
    cargo build --release --locked --target x86_64-pc-windows-msvc --manifest-path=./app/Cargo.toml ${cargo_features}; \
    cp "./app/target/x86_64-pc-windows-msvc/release/${cargo_name}.exe" "$output"; \
    ls -lh "$output"

FROM scratch AS artifact

COPY --from=build /var/workdir/out/ /
