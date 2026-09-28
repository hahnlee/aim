#pragma once

// Resolves the libandroid NDK asset API that is backed by the same
// AssetManager2 instance used by android.content.res.AssetManager.
void* aim_android_asset_manager_symbol(const char* symbol);
