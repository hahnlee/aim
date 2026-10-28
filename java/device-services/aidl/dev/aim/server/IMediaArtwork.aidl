package dev.aim.server;

/** Original Android image decoding for the system UID's Now Playing bridge. */
interface IMediaArtwork {
    byte[] load(String uri);
}
