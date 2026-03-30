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

import com.shieldblaze.expressgateway.protocol.quic.ByteArrayKey;
import com.shieldblaze.expressgateway.protocol.quic.QuicBackendSession;
import com.shieldblaze.expressgateway.protocol.quic.QuicCidSessionMap;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.time.Duration;
import java.util.Arrays;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * CID-based router for multi-instance QUIC load balancing.
 *
 * <p>Routes incoming QUIC packets to the correct backend session by extracting
 * the server ID prefix from the Destination Connection ID (DCID). In a
 * multi-instance deployment, the CID prefix identifies which load balancer
 * instance owns the connection, enabling stateless packet routing.</p>
 *
 * <h3>Routing Algorithm</h3>
 * <ol>
 *   <li>Extract server ID prefix from DCID (first 4 bytes)</li>
 *   <li>Check if prefix matches this instance ({@link CidGenerator#isOwnCid})</li>
 *   <li>If own: look up in local {@link QuicCidSessionMap}</li>
 *   <li>If foreign: look up target instance in the server registry</li>
 * </ol>
 *
 * <h3>CID Rotation (RFC 9000 Section 5.1)</h3>
 * <p>QUIC endpoints can rotate CIDs via NEW_CONNECTION_ID frames. When the proxy
 * generates a new CID for an existing connection, it must:
 * <ul>
 *   <li>Use the same server ID prefix (so routing stays consistent)</li>
 *   <li>Register the new CID in the session map</li>
 *   <li>Optionally retire old CIDs via RETIRE_CONNECTION_ID</li>
 * </ul>
 * This class tracks all active CIDs per connection for rotation support.</p>
 *
 * <h3>Thread Safety</h3>
 * <p>All operations are lock-free using {@link ConcurrentHashMap}. The server
 * registry is write-rarely/read-often, optimized for lookup performance.</p>
 */
public final class CidRouter implements Closeable {

    private static final Logger logger = LogManager.getLogger(CidRouter.class);

    /**
     * Result of a CID routing decision.
     *
     * @param session    the backend session if found locally, or null
     * @param ownedByUs true if the CID prefix matches this instance
     * @param targetServerPrefix the server ID prefix extracted from the CID, for forwarding decisions
     */
    public record RoutingResult(QuicBackendSession session, boolean ownedByUs, byte[] targetServerPrefix) {

        /** No match: CID is unknown or unparseable. */
        static final RoutingResult NO_MATCH = new RoutingResult(null, false, null);

        /** True if a local session was found. */
        public boolean hasSession() {
            return session != null;
        }
    }

    private final CidGenerator cidGenerator;
    private final QuicCidSessionMap cidSessionMap;

    /**
     * Maps each CID to the primary CID of the connection it belongs to.
     * The primary CID is the first CID registered for a connection.
     * Used for CID rotation: when a new CID is issued, it maps to the same
     * primary CID. When a CID is retired, only that specific mapping
     * is removed, not the entire connection.
     */
    private final ConcurrentHashMap<ByteArrayKey, ByteArrayKey> cidToConnectionKey = new ConcurrentHashMap<>();

    /**
     * Reverse map: primary CID -> set of all CIDs belonging to that connection.
     * Enables retiring all CIDs for a connection in one operation.
     */
    private final ConcurrentHashMap<ByteArrayKey, Set<ByteArrayKey>> connectionCids = new ConcurrentHashMap<>();

    /**
     * Registry of known server ID prefixes for forwarding decisions.
     * Maps server ID prefix -> server identifier (e.g., hostname:port).
     * Populated externally via {@link #registerServer(byte[], String)}.
     */
    private final ConcurrentHashMap<ByteArrayKey, String> serverRegistry = new ConcurrentHashMap<>();

    /** Total CID lookups for observability. */
    private final AtomicLong totalLookups = new AtomicLong();
    /** CID lookups that resolved to a local session. */
    private final AtomicLong localHits = new AtomicLong();
    /** CID lookups where the prefix identified a foreign instance. */
    private final AtomicLong foreignHits = new AtomicLong();

    /**
     * Create a new CID router.
     *
     * @param cidGenerator  the CID generator for this instance
     * @param cidSessionMap the CID-to-session map for local session lookups
     */
    public CidRouter(CidGenerator cidGenerator, QuicCidSessionMap cidSessionMap) {
        this.cidGenerator = Objects.requireNonNull(cidGenerator, "cidGenerator");
        this.cidSessionMap = Objects.requireNonNull(cidSessionMap, "cidSessionMap");
    }

    /**
     * Route an incoming packet by its Destination Connection ID.
     *
     * @param dcid the DCID extracted from the QUIC packet header
     * @return the routing result indicating where this packet should go
     */
    public RoutingResult route(byte[] dcid) {
        if (dcid == null || dcid.length < CidGenerator.SERVER_ID_PREFIX_LEN) {
            return RoutingResult.NO_MATCH;
        }

        totalLookups.incrementAndGet();
        byte[] prefix = CidGenerator.extractServerIdPrefix(dcid);

        if (cidGenerator.isOwnCid(dcid)) {
            // CID belongs to this instance -- look up in local session map
            QuicBackendSession session = cidSessionMap.get(dcid);
            if (session != null) {
                localHits.incrementAndGet();
            }
            return new RoutingResult(session, true, prefix);
        }

        // CID belongs to a different instance
        foreignHits.incrementAndGet();
        return new RoutingResult(null, false, prefix);
    }

    /**
     * Generate a new CID and register it for an existing session (CID rotation).
     *
     * <p>Per RFC 9000 Section 5.1, endpoints issue new CIDs via NEW_CONNECTION_ID
     * and retire old ones via RETIRE_CONNECTION_ID. The new CID retains the same
     * server ID prefix to ensure consistent routing.</p>
     *
     * @param session    the backend session this CID will route to
     * @param primaryCid the primary (first) CID for this connection, used to associate the new CID
     * @return the newly generated CID
     */
    public byte[] issueNewCid(QuicBackendSession session, byte[] primaryCid) {
        byte[] newCid = cidGenerator.generate();
        cidSessionMap.put(newCid, session, cidGenerator.cidLength());

        ByteArrayKey newKey = new ByteArrayKey(newCid);
        ByteArrayKey primaryKey = new ByteArrayKey(primaryCid);
        cidToConnectionKey.put(newKey, primaryKey);
        connectionCids.computeIfAbsent(primaryKey, k -> ConcurrentHashMap.newKeySet()).add(newKey);

        if (logger.isDebugEnabled()) {
            logger.debug("Issued new CID (rotation) for session to {}",
                    session.node().socketAddress());
        }
        return newCid;
    }

    /**
     * Generate a new CID and register it for an existing session (CID rotation).
     * Uses the new CID itself as the primary CID (legacy convenience overload).
     *
     * @param session the backend session this CID will route to
     * @return the newly generated CID
     */
    public byte[] issueNewCid(QuicBackendSession session) {
        byte[] newCid = cidGenerator.generate();
        cidSessionMap.put(newCid, session, cidGenerator.cidLength());

        ByteArrayKey newKey = new ByteArrayKey(newCid);
        cidToConnectionKey.put(newKey, newKey);
        connectionCids.computeIfAbsent(newKey, k -> ConcurrentHashMap.newKeySet()).add(newKey);

        if (logger.isDebugEnabled()) {
            logger.debug("Issued new CID (rotation) for session to {}",
                    session.node().socketAddress());
        }
        return newCid;
    }

    /**
     * Register an initial CID for a new connection. The CID is used as its own
     * primary CID (first CID for the connection).
     *
     * @param cid        the CID to register
     * @param session    the backend session
     * @param cidLength  the CID length
     */
    public void registerCid(byte[] cid, QuicBackendSession session, int cidLength) {
        registerCid(cid, session, cidLength, cid);
    }

    /**
     * Register a CID for a connection with an explicit primary CID for association.
     *
     * @param cid        the CID to register
     * @param session    the backend session
     * @param cidLength  the CID length
     * @param primaryCid the primary (first) CID for the connection
     */
    public void registerCid(byte[] cid, QuicBackendSession session, int cidLength, byte[] primaryCid) {
        cidSessionMap.put(cid, session, cidLength);
        ByteArrayKey cidKey = new ByteArrayKey(cid);
        ByteArrayKey primaryKey = new ByteArrayKey(primaryCid);
        cidToConnectionKey.put(cidKey, primaryKey);
        connectionCids.computeIfAbsent(primaryKey, k -> ConcurrentHashMap.newKeySet()).add(cidKey);
    }

    /**
     * Retire a CID (RFC 9000 Section 5.1.2).
     *
     * <p>Removes the CID mapping from both the session map and the connection tracking
     * maps. Does NOT close the session. Other CIDs for the same connection remain active.</p>
     *
     * @param cid the CID to retire
     */
    public void retireCid(byte[] cid) {
        cidSessionMap.remove(cid);
        ByteArrayKey cidKey = new ByteArrayKey(cid);
        ByteArrayKey primaryKey = cidToConnectionKey.remove(cidKey);
        if (primaryKey != null) {
            Set<ByteArrayKey> allCids = connectionCids.get(primaryKey);
            if (allCids != null) {
                allCids.remove(cidKey);
                if (allCids.isEmpty()) {
                    connectionCids.remove(primaryKey);
                }
            }
        }

        if (logger.isDebugEnabled()) {
            logger.debug("Retired CID: {}", Arrays.toString(cid));
        }
    }

    /**
     * Retire all CIDs associated with a connection identified by its primary CID.
     * Removes all CID mappings from both the session map and the connection tracking maps.
     *
     * @param primaryCid the primary (first) CID that identifies the connection
     */
    public void retireConnection(byte[] primaryCid) {
        ByteArrayKey primaryKey = new ByteArrayKey(primaryCid);
        Set<ByteArrayKey> allCids = connectionCids.remove(primaryKey);
        if (allCids != null) {
            for (ByteArrayKey cidKey : allCids) {
                cidSessionMap.remove(cidKey.data());
                cidToConnectionKey.remove(cidKey);
            }
        }

        if (logger.isDebugEnabled()) {
            logger.debug("Retired connection (all CIDs) for primary CID: {}", Arrays.toString(primaryCid));
        }
    }

    /**
     * Returns the primary CID for a given CID, or null if the CID is not tracked.
     *
     * @param cid the CID to look up
     * @return the primary CID key, or null
     */
    public ByteArrayKey getPrimaryCid(byte[] cid) {
        return cidToConnectionKey.get(new ByteArrayKey(cid));
    }

    /**
     * Returns all CIDs associated with a connection identified by its primary CID.
     *
     * @param primaryCid the primary CID
     * @return the set of all CIDs for this connection, or null if not tracked
     */
    public Set<ByteArrayKey> getConnectionCids(byte[] primaryCid) {
        return connectionCids.get(new ByteArrayKey(primaryCid));
    }

    /**
     * Register a peer server instance for foreign CID forwarding.
     *
     * @param serverIdPrefix the server's ID prefix (from {@link CidGenerator#serverIdPrefix()})
     * @param serverIdentifier a human-readable identifier (e.g., "lb-02:10.0.1.2:4433")
     */
    public void registerServer(byte[] serverIdPrefix, String serverIdentifier) {
        serverRegistry.put(new ByteArrayKey(serverIdPrefix), serverIdentifier);
    }

    /**
     * Deregister a peer server instance.
     *
     * @param serverIdPrefix the server's ID prefix to remove
     */
    public void deregisterServer(byte[] serverIdPrefix) {
        serverRegistry.remove(new ByteArrayKey(serverIdPrefix));
    }

    /**
     * Look up the server identifier for a given server ID prefix.
     *
     * @param serverIdPrefix the extracted server ID prefix from a CID
     * @return the server identifier, or null if unknown
     */
    public String resolveServer(byte[] serverIdPrefix) {
        if (serverIdPrefix == null) return null;
        return serverRegistry.get(new ByteArrayKey(serverIdPrefix));
    }

    /**
     * Returns the underlying CID generator.
     */
    public CidGenerator cidGenerator() {
        return cidGenerator;
    }

    /**
     * Returns the total number of CID lookups performed.
     */
    public long totalLookups() {
        return totalLookups.get();
    }

    /**
     * Returns the number of lookups resolved to a local session.
     */
    public long localHits() {
        return localHits.get();
    }

    /**
     * Returns the number of lookups identified as foreign instance CIDs.
     */
    public long foreignHits() {
        return foreignHits.get();
    }

    @Override
    public void close() {
        cidToConnectionKey.clear();
        connectionCids.clear();
        serverRegistry.clear();
    }
}
