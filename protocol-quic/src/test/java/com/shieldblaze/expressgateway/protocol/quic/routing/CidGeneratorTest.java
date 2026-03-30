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
package com.shieldblaze.expressgateway.protocol.quic.routing;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.security.SecureRandom;
import java.util.HashSet;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link CidGenerator} -- cryptographic CID generation with
 * embedded server-ID routing prefix.
 */
@Timeout(10)
class CidGeneratorTest {

    private static final SecureRandom SECURE_RANDOM = new SecureRandom();
    private static final byte[] SERVER_ID = "server-01".getBytes();
    private static final byte[] CLUSTER_SECRET = new byte[32];
    static {
        SECURE_RANDOM.nextBytes(CLUSTER_SECRET);
    }

    @Test
    void generate_producesCorrectLength() {
        for (int len = CidGenerator.MIN_CID_LENGTH; len <= CidGenerator.MAX_CID_LENGTH; len++) {
            CidGenerator gen = new CidGenerator(SERVER_ID, CLUSTER_SECRET, len);
            byte[] cid = gen.generate();
            assertEquals(len, cid.length, "Generated CID must be exactly " + len + " bytes");
        }
    }

    @Test
    void generate_embedsConsistentServerIdPrefix() {
        CidGenerator gen = new CidGenerator(SERVER_ID, CLUSTER_SECRET, 8);
        byte[] cid1 = gen.generate();
        byte[] cid2 = gen.generate();

        // First 4 bytes (server ID prefix) must be identical across all CIDs from same generator
        byte[] prefix1 = CidGenerator.extractServerIdPrefix(cid1);
        byte[] prefix2 = CidGenerator.extractServerIdPrefix(cid2);
        assertNotNull(prefix1);
        assertNotNull(prefix2);
        assertArrayEquals(prefix1, prefix2, "Server ID prefix must be consistent across CIDs");
    }

    @Test
    void generate_randomSuffixDiffers() {
        CidGenerator gen = new CidGenerator(SERVER_ID, CLUSTER_SECRET, 20);
        Set<String> cidSet = new HashSet<>();
        for (int i = 0; i < 1000; i++) {
            byte[] cid = gen.generate();
            cidSet.add(java.util.HexFormat.of().formatHex(cid));
        }
        // All 1000 CIDs must be unique (random suffix collision at 20 bytes is negligible)
        assertEquals(1000, cidSet.size(), "All generated CIDs must be unique");
    }

    @Test
    void isOwnCid_matchesOwnCids() {
        CidGenerator gen = new CidGenerator(SERVER_ID, CLUSTER_SECRET, 8);
        byte[] cid = gen.generate();
        assertTrue(gen.isOwnCid(cid), "Generator must recognize its own CIDs");
    }

    @Test
    void isOwnCid_rejectsForeignCids() {
        CidGenerator gen1 = new CidGenerator("server-01".getBytes(), CLUSTER_SECRET, 8);
        CidGenerator gen2 = new CidGenerator("server-02".getBytes(), CLUSTER_SECRET, 8);
        byte[] cidFromGen2 = gen2.generate();
        assertFalse(gen1.isOwnCid(cidFromGen2), "Must reject CIDs from a different server");
    }

    @Test
    void isOwnCid_rejectsShortCid() {
        CidGenerator gen = new CidGenerator(SERVER_ID, CLUSTER_SECRET, 8);
        assertFalse(gen.isOwnCid(new byte[]{0x01}), "Must reject CIDs shorter than prefix length");
        assertFalse(gen.isOwnCid(null), "Must reject null CID");
        assertFalse(gen.isOwnCid(new byte[0]), "Must reject empty CID");
    }

    @Test
    void computeServerIdPrefix_matchesGeneratorPrefix() {
        CidGenerator gen = new CidGenerator(SERVER_ID, CLUSTER_SECRET, 8);
        byte[] computedPrefix = CidGenerator.computeServerIdPrefix(SERVER_ID, CLUSTER_SECRET);
        byte[] generatorPrefix = gen.serverIdPrefix();
        assertArrayEquals(computedPrefix, generatorPrefix,
                "computeServerIdPrefix must match the generator's prefix");
    }

    @Test
    void extractServerIdPrefix_returnsFirst4Bytes() {
        byte[] cid = {0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08};
        byte[] prefix = CidGenerator.extractServerIdPrefix(cid);
        assertNotNull(prefix);
        assertArrayEquals(new byte[]{0x01, 0x02, 0x03, 0x04}, prefix);
    }

    @Test
    void extractServerIdPrefix_returnsNullForShortCid() {
        assertNull(CidGenerator.extractServerIdPrefix(new byte[]{0x01, 0x02}));
        assertNull(CidGenerator.extractServerIdPrefix(null));
    }

    @Test
    void constructor_rejectsBelowMinCidLength() {
        assertThrows(IllegalArgumentException.class,
                () -> new CidGenerator(SERVER_ID, CLUSTER_SECRET, 3),
                "CID length below 4 must be rejected");
    }

    @Test
    void constructor_rejectsAboveMaxCidLength() {
        assertThrows(IllegalArgumentException.class,
                () -> new CidGenerator(SERVER_ID, CLUSTER_SECRET, 21),
                "CID length above 20 must be rejected");
    }

    @Test
    void constructor_rejectsShortClusterSecret() {
        assertThrows(IllegalArgumentException.class,
                () -> new CidGenerator(SERVER_ID, new byte[8], 8),
                "Cluster secret below 16 bytes must be rejected");
    }

    @Test
    void differentClusterSecrets_produceDifferentPrefixes() {
        byte[] secret1 = new byte[16];
        byte[] secret2 = new byte[16];
        SECURE_RANDOM.nextBytes(secret1);
        SECURE_RANDOM.nextBytes(secret2);

        CidGenerator gen1 = new CidGenerator(SERVER_ID, secret1, 8);
        CidGenerator gen2 = new CidGenerator(SERVER_ID, secret2, 8);

        assertFalse(gen1.isOwnCid(gen2.generate()),
                "Different cluster secrets must produce different prefixes");
    }

    @Test
    void cidLength_returnsConfiguredLength() {
        CidGenerator gen = new CidGenerator(SERVER_ID, CLUSTER_SECRET, 12);
        assertEquals(12, gen.cidLength());
    }

    @Test
    void minimalCidLength_generatesValidCid() {
        // Minimum CID length = 4 (all prefix, no random suffix)
        CidGenerator gen = new CidGenerator(SERVER_ID, CLUSTER_SECRET, 4);
        byte[] cid = gen.generate();
        assertEquals(4, cid.length);
        assertTrue(gen.isOwnCid(cid));
    }
}
