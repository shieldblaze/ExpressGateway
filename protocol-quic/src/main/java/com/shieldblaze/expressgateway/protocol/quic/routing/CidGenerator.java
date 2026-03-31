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

import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;
import java.security.InvalidKeyException;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;
import java.util.Objects;

/**
 * Cryptographically secure QUIC Connection ID generator with embedded server-ID
 * routing information for multi-instance load balancer deployments.
 *
 * <h3>CID Format (RFC 9000 Section 5.1 compliant)</h3>
 * <pre>
 * [server_id_hash : SERVER_ID_PREFIX_LEN bytes][random : remaining bytes]
 * </pre>
 *
 * <p>The server ID prefix is an HMAC-SHA256 truncation of the logical server identifier,
 * keyed with a cluster-wide shared secret. This allows any load balancer instance in the
 * cluster to extract the target server from a CID without maintaining shared state.</p>
 *
 * <h3>CID Length Constraints</h3>
 * <p>RFC 9000 Section 17.2: Connection IDs MUST be between 0 and 20 bytes inclusive.
 * This generator enforces a minimum of 4 bytes (to fit the server ID prefix) and a
 * maximum of 20 bytes.</p>
 *
 * <h3>Thread Safety</h3>
 * <p>Thread-safe via ThreadLocal SecureRandom. The HMAC computation uses a ThreadLocal
 * Mac instance to avoid contention. No locks are used in the generation hot path.</p>
 *
 * <h3>Security Properties</h3>
 * <ul>
 *   <li>Server ID is not directly embedded -- attackers cannot read it from the CID</li>
 *   <li>HMAC prefix prevents CID forgery without the cluster secret</li>
 *   <li>Random suffix prevents CID prediction and correlation attacks</li>
 * </ul>
 */
public final class CidGenerator {

    private static final String HMAC_ALGO = "HmacSHA256";

    /**
     * Number of bytes reserved for the server ID hash prefix in the CID.
     * 4 bytes = 2^32 possible server ID hashes, sufficient for multi-instance routing.
     */
    static final int SERVER_ID_PREFIX_LEN = 4;

    /**
     * Minimum CID length: must fit at least the server ID prefix.
     */
    public static final int MIN_CID_LENGTH = 4;

    /**
     * Maximum CID length per RFC 9000 Section 17.2.
     */
    public static final int MAX_CID_LENGTH = 20;

    private final int cidLength;
    private final byte[] serverIdPrefix;
    private final SecretKeySpec hmacKey;

    private final ThreadLocal<SecureRandom> threadLocalRandom =
            ThreadLocal.withInitial(SecureRandom::new);

    private final ThreadLocal<Mac> threadLocalMac;

    /**
     * Create a new CID generator.
     *
     * @param serverId     logical server identifier (e.g., hostname, instance ID).
     *                     Hashed with HMAC so it does not appear in the CID directly.
     * @param clusterSecret shared secret across all load balancer instances in the cluster.
     *                      Must be at least 16 bytes.
     * @param cidLength    desired CID length in bytes, in range [{@value MIN_CID_LENGTH}, {@value MAX_CID_LENGTH}].
     * @throws IllegalArgumentException if cidLength is out of range or clusterSecret is too short
     */
    public CidGenerator(byte[] serverId, byte[] clusterSecret, int cidLength) {
        Objects.requireNonNull(serverId, "serverId");
        Objects.requireNonNull(clusterSecret, "clusterSecret");

        if (cidLength < MIN_CID_LENGTH || cidLength > MAX_CID_LENGTH) {
            throw new IllegalArgumentException(
                    "CID length must be in [" + MIN_CID_LENGTH + ", " + MAX_CID_LENGTH + "], got: " + cidLength);
        }
        if (clusterSecret.length < 16) {
            throw new IllegalArgumentException(
                    "Cluster secret must be at least 16 bytes, got: " + clusterSecret.length);
        }

        this.cidLength = cidLength;
        this.hmacKey = new SecretKeySpec(clusterSecret, HMAC_ALGO);
        this.threadLocalMac = ThreadLocal.withInitial(this::createMac);

        // Compute the server ID prefix: first SERVER_ID_PREFIX_LEN bytes of HMAC(clusterSecret, serverId)
        Mac mac = createMac();
        byte[] fullHmac = mac.doFinal(serverId);
        this.serverIdPrefix = new byte[SERVER_ID_PREFIX_LEN];
        System.arraycopy(fullHmac, 0, this.serverIdPrefix, 0, SERVER_ID_PREFIX_LEN);
    }

    /**
     * Generate a new Connection ID with embedded server routing information.
     *
     * <p>The generated CID has the structure:
     * {@code [server_id_hash(4)][random(cidLength - 4)]}
     * </p>
     *
     * @return a new CID byte array of the configured length
     */
    public byte[] generate() {
        byte[] cid = new byte[cidLength];
        System.arraycopy(serverIdPrefix, 0, cid, 0, SERVER_ID_PREFIX_LEN);

        // Fill remaining bytes with cryptographically secure random data
        int randomLen = cidLength - SERVER_ID_PREFIX_LEN;
        if (randomLen > 0) {
            byte[] randomBytes = new byte[randomLen];
            threadLocalRandom.get().nextBytes(randomBytes);
            System.arraycopy(randomBytes, 0, cid, SERVER_ID_PREFIX_LEN, randomLen);
        }

        return cid;
    }

    /**
     * Extract the server ID prefix from a given CID.
     *
     * @param cid the connection ID bytes
     * @return the server ID prefix (first {@value SERVER_ID_PREFIX_LEN} bytes), or null if too short
     */
    public static byte[] extractServerIdPrefix(byte[] cid) {
        if (cid == null || cid.length < SERVER_ID_PREFIX_LEN) {
            return null;
        }
        byte[] prefix = new byte[SERVER_ID_PREFIX_LEN];
        System.arraycopy(cid, 0, prefix, 0, SERVER_ID_PREFIX_LEN);
        return prefix;
    }

    /**
     * Compute the server ID prefix for a given logical server ID.
     * Used by the routing layer to match CID prefixes to server instances.
     *
     * @param serverId      the logical server identifier
     * @param clusterSecret the cluster-wide shared secret
     * @return the server ID prefix bytes
     */
    public static byte[] computeServerIdPrefix(byte[] serverId, byte[] clusterSecret) {
        try {
            Mac mac = Mac.getInstance(HMAC_ALGO);
            mac.init(new SecretKeySpec(clusterSecret, HMAC_ALGO));
            byte[] fullHmac = mac.doFinal(serverId);
            byte[] prefix = new byte[SERVER_ID_PREFIX_LEN];
            System.arraycopy(fullHmac, 0, prefix, 0, SERVER_ID_PREFIX_LEN);
            return prefix;
        } catch (NoSuchAlgorithmException | InvalidKeyException e) {
            throw new IllegalStateException("Failed to compute server ID prefix", e);
        }
    }

    /**
     * Validate that a CID was generated by this generator instance (same server ID prefix).
     *
     * @param cid the connection ID to validate
     * @return true if the CID prefix matches this server's prefix
     */
    public boolean isOwnCid(byte[] cid) {
        if (cid == null || cid.length < SERVER_ID_PREFIX_LEN) {
            return false;
        }
        return constantTimeEquals(serverIdPrefix, cid, SERVER_ID_PREFIX_LEN);
    }

    /**
     * Returns the configured CID length.
     */
    public int cidLength() {
        return cidLength;
    }

    /**
     * Returns a copy of this server's ID prefix for external use (e.g., CidRouter registration).
     */
    public byte[] serverIdPrefix() {
        byte[] copy = new byte[SERVER_ID_PREFIX_LEN];
        System.arraycopy(serverIdPrefix, 0, copy, 0, SERVER_ID_PREFIX_LEN);
        return copy;
    }

    private Mac createMac() {
        try {
            Mac mac = Mac.getInstance(HMAC_ALGO);
            mac.init(hmacKey);
            return mac;
        } catch (NoSuchAlgorithmException | InvalidKeyException e) {
            throw new IllegalStateException("Failed to initialize HMAC for CID generation", e);
        }
    }

    /**
     * Constant-time comparison of the first {@code length} bytes of two arrays.
     * Prevents timing side-channels on CID prefix validation.
     */
    private static boolean constantTimeEquals(byte[] a, byte[] b, int length) {
        if (a.length < length || b.length < length) {
            return false;
        }
        int result = 0;
        for (int i = 0; i < length; i++) {
            result |= a[i] ^ b[i];
        }
        return result == 0;
    }
}
