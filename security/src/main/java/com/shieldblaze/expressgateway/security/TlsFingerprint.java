/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.security;

import io.netty.buffer.ByteBuf;

import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CopyOnWriteArraySet;

/**
 * JA3/JA3S TLS fingerprint extraction from ClientHello messages.
 *
 * <p>JA3 fingerprints are computed from the TLS ClientHello by concatenating:
 * TLSVersion, Ciphers, Extensions, EllipticCurves, EllipticCurvePointFormats
 * separated by commas, then MD5-hashing the result.</p>
 *
 * <p>This record is immutable and represents a single extracted fingerprint.
 * Use {@link #fromClientHello(ByteBuf)} to extract from a raw TLS record.</p>
 *
 * @param tlsVersion        TLS version from ClientHello (e.g., 771 for TLS 1.2)
 * @param cipherSuites      List of cipher suite identifiers
 * @param extensions         List of extension type identifiers
 * @param ellipticCurves     List of supported group identifiers (from supported_groups extension)
 * @param ecPointFormats     List of EC point format identifiers
 * @param ja3String          The raw JA3 string before hashing
 * @param ja3Hash            MD5 hash of the JA3 string (the actual fingerprint)
 */
public record TlsFingerprint(
        int tlsVersion,
        List<Integer> cipherSuites,
        List<Integer> extensions,
        List<Integer> ellipticCurves,
        List<Integer> ecPointFormats,
        String ja3String,
        String ja3Hash
) {

    // GREASE values defined in RFC 8701 -- must be excluded from JA3 computation
    private static final Set<Integer> GREASE_VALUES = Set.of(
            0x0a0a, 0x1a1a, 0x2a2a, 0x3a3a, 0x4a4a, 0x5a5a, 0x6a6a, 0x7a7a,
            0x8a8a, 0x9a9a, 0xaaaa, 0xbaba, 0xcaca, 0xdada, 0xeaea, 0xfafa
    );

    private static final HexFormat HEX = HexFormat.of();

    /**
     * Extract a JA3 fingerprint from a TLS ClientHello record.
     *
     * <p>The ByteBuf must be positioned at the start of the TLS record
     * (Content Type byte). The reader index is not modified.</p>
     *
     * @param buf the ByteBuf containing the TLS ClientHello record
     * @return the extracted fingerprint, or null if the buffer is not a valid ClientHello
     */
    public static TlsFingerprint fromClientHello(ByteBuf buf) {
        if (buf == null || buf.readableBytes() < 44) {
            return null; // minimum ClientHello size
        }

        int readerIdx = buf.readerIndex();
        try {
            // TLS Record Layer
            int contentType = buf.readUnsignedByte();
            if (contentType != 22) { // Handshake
                return null;
            }

            buf.readUnsignedShort(); // Record layer version (consumed but not needed)
            int recordLength = buf.readUnsignedShort();

            if (buf.readableBytes() < recordLength) {
                return null;
            }

            // Handshake Header
            int handshakeType = buf.readUnsignedByte();
            if (handshakeType != 1) { // ClientHello
                return null;
            }

            // Handshake length (3 bytes)
            buf.readUnsignedMedium();

            // ClientHello fields
            int clientVersion = buf.readUnsignedShort(); // TLS version used in JA3

            // Random (32 bytes)
            buf.skipBytes(32);

            // Session ID
            int sessionIdLen = buf.readUnsignedByte();
            buf.skipBytes(sessionIdLen);

            // Cipher Suites
            int cipherSuitesLen = buf.readUnsignedShort();
            List<Integer> ciphers = new ArrayList<>(cipherSuitesLen / 2);
            for (int i = 0; i < cipherSuitesLen; i += 2) {
                int cs = buf.readUnsignedShort();
                if (!GREASE_VALUES.contains(cs)) {
                    ciphers.add(cs);
                }
            }

            // Compression Methods
            int compressionLen = buf.readUnsignedByte();
            buf.skipBytes(compressionLen);

            List<Integer> extList = new ArrayList<>();
            List<Integer> curves = new ArrayList<>();
            List<Integer> pointFormats = new ArrayList<>();

            // Extensions (if present)
            if (buf.readableBytes() >= 2) {
                int extensionsLen = buf.readUnsignedShort();
                int extensionsEnd = buf.readerIndex() + extensionsLen;

                while (buf.readerIndex() < extensionsEnd && buf.readableBytes() >= 4) {
                    int extType = buf.readUnsignedShort();
                    int extLen = buf.readUnsignedShort();
                    int extDataEnd = buf.readerIndex() + extLen;

                    if (!GREASE_VALUES.contains(extType)) {
                        extList.add(extType);

                        // Supported Groups (formerly Elliptic Curves) - extension type 10
                        if (extType == 10 && extLen >= 2) {
                            int groupsLen = buf.readUnsignedShort();
                            for (int i = 0; i < groupsLen; i += 2) {
                                int group = buf.readUnsignedShort();
                                if (!GREASE_VALUES.contains(group)) {
                                    curves.add(group);
                                }
                            }
                        }
                        // EC Point Formats - extension type 11
                        else if (extType == 11 && extLen >= 1) {
                            int formatsLen = buf.readUnsignedByte();
                            for (int i = 0; i < formatsLen; i++) {
                                pointFormats.add((int) buf.readUnsignedByte());
                            }
                        }
                    }

                    buf.readerIndex(extDataEnd);
                }
            }

            // Build JA3 string: TLSVersion,Ciphers,Extensions,EllipticCurves,ECPointFormats
            StringBuilder ja3 = new StringBuilder();
            ja3.append(clientVersion);
            ja3.append(',');
            appendIntList(ja3, ciphers);
            ja3.append(',');
            appendIntList(ja3, extList);
            ja3.append(',');
            appendIntList(ja3, curves);
            ja3.append(',');
            appendIntList(ja3, pointFormats);

            String ja3Str = ja3.toString();
            String hash = md5Hex(ja3Str);

            return new TlsFingerprint(clientVersion, List.copyOf(ciphers), List.copyOf(extList),
                    List.copyOf(curves), List.copyOf(pointFormats), ja3Str, hash);
        } catch (Exception e) {
            return null; // Malformed ClientHello
        } finally {
            buf.readerIndex(readerIdx);
        }
    }

    private static void appendIntList(StringBuilder sb, List<Integer> values) {
        for (int i = 0; i < values.size(); i++) {
            if (i > 0) sb.append('-');
            sb.append(values.get(i));
        }
    }

    /**
     * JA3 fingerprint hash algorithm. The JA3 specification
     * (<a href="https://github.com/salesforce/ja3">salesforce/ja3</a>)
     * mandates MD5 for the fingerprint hash. This is NOT used for
     * cryptographic security -- it is a fixed protocol identifier.
     */
    private static final String JA3_HASH_ALGORITHM = "MD5";

    private static String md5Hex(String input) {
        try {
            MessageDigest md = MessageDigest.getInstance(ja3HashAlgorithm());
            byte[] digest = md.digest(input.getBytes());
            return HEX.formatHex(digest);
        } catch (NoSuchAlgorithmException e) {
            throw new AssertionError(JA3_HASH_ALGORITHM + " not available", e);
        }
    }

    /**
     * Returns the hash algorithm mandated by the JA3 specification.
     * Extracted to a method to document that this is a protocol requirement,
     * not a free choice of algorithm.
     */
    private static String ja3HashAlgorithm() {
        return JA3_HASH_ALGORITHM;
    }

    /**
     * A thread-safe registry of known-bad JA3 fingerprint hashes.
     * Used to block connections from known malicious TLS implementations.
     */
    public static final class BlockList {
        private final CopyOnWriteArraySet<String> blockedHashes = new CopyOnWriteArraySet<>();

        /**
         * Add a JA3 hash to the block list.
         */
        public void add(String ja3Hash) {
            blockedHashes.add(ja3Hash);
        }

        /**
         * Remove a JA3 hash from the block list.
         */
        public void remove(String ja3Hash) {
            blockedHashes.remove(ja3Hash);
        }

        /**
         * Check if a fingerprint is blocked.
         */
        public boolean isBlocked(TlsFingerprint fingerprint) {
            return fingerprint != null && blockedHashes.contains(fingerprint.ja3Hash());
        }

        /**
         * Check if a JA3 hash is blocked.
         */
        public boolean isBlocked(String ja3Hash) {
            return blockedHashes.contains(ja3Hash);
        }

        /**
         * Returns the number of blocked hashes.
         */
        public int size() {
            return blockedHashes.size();
        }

        /**
         * Clear all blocked hashes.
         */
        public void clear() {
            blockedHashes.clear();
        }
    }
}
