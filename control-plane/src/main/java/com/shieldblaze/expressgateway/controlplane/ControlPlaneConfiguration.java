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
package com.shieldblaze.expressgateway.controlplane;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.controlplane.kvstore.StorageConfiguration;

import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/**
 * Configuration for the Control Plane server.
 *
 * <p>All fields have sensible defaults. After construction (or deserialization),
 * call {@link #validate()} to verify that all values are within acceptable ranges.
 * Invalid configurations throw {@link IllegalArgumentException} listing every
 * violation, not just the first one found.</p>
 *
 * <p>Immutability note: this class uses mutable fields for Jackson deserialization
 * compatibility. Once validated, it should be treated as effectively immutable --
 * do not modify fields after passing the configuration to {@link ControlPlaneServer}.</p>
 */
public final class ControlPlaneConfiguration {

    @JsonProperty("GrpcPort")
    private int grpcPort = 9443;

    @JsonProperty("GrpcBindAddress")
    private String grpcBindAddress = "0.0.0.0";

    @JsonProperty("KvStoreType")
    private KvStoreType kvStoreType = KvStoreType.ZOOKEEPER;

    @JsonProperty("HeartbeatIntervalMs")
    private long heartbeatIntervalMs = 10_000;

    @JsonProperty("HeartbeatMissThreshold")
    private int heartbeatMissThreshold = 3;

    @JsonProperty("HeartbeatDisconnectThreshold")
    private int heartbeatDisconnectThreshold = 6;

    @JsonProperty("HeartbeatScanIntervalMs")
    private long heartbeatScanIntervalMs = 5000;

    @JsonProperty("WriteBatchWindowMs")
    private long writeBatchWindowMs = 500;

    @JsonProperty("MaxJournalLag")
    private long maxJournalLag = 10_000;

    @JsonProperty("MaxNodes")
    private int maxNodes = 10_000;

    @JsonProperty("MaxRequestsPerSecondPerNode")
    private int maxRequestsPerSecondPerNode = 100;

    @JsonProperty("Region")
    private String region = "default";

    @JsonProperty("ClusterEnabled")
    private boolean clusterEnabled = false;

    @JsonProperty("ReconnectBurst")
    private int reconnectBurst = 500;

    @JsonProperty("ReconnectRefillRate")
    private int reconnectRefillRate = 100;

    @JsonProperty("GrpcTlsEnabled")
    private boolean grpcTlsEnabled = false;

    @JsonProperty("GrpcTlsCertPath")
    private String grpcTlsCertPath;

    @JsonProperty("GrpcTlsKeyPath")
    private String grpcTlsKeyPath;

    @JsonProperty("GrpcTlsCaPath")
    private String grpcTlsCaPath;

    @JsonProperty("Storage")
    private StorageConfiguration storage = new StorageConfiguration();

    /**
     * Supported KV store backends for config persistence and journal storage.
     */
    public enum KvStoreType {
        ZOOKEEPER,
        CONSUL,
        ETCD
    }

    /**
     * Default constructor for Jackson deserialization.
     * All fields are initialized to their defaults.
     */
    public ControlPlaneConfiguration() {
    }

    /**
     * Validates all configuration values and throws {@link IllegalArgumentException}
     * if any are out of range.
     *
     * <p>Collects all violations before throwing, so callers see every problem
     * at once rather than fixing them one at a time.</p>
     *
     * @return this configuration instance (for fluent chaining)
     * @throws IllegalArgumentException if one or more values are invalid
     */
    public ControlPlaneConfiguration validate() {
        List<String> violations = new ArrayList<>();

        if (grpcPort < 0 || grpcPort > 65535) {
            violations.add("GrpcPort must be in range [0, 65535], got: " + grpcPort);
        }
        if (grpcBindAddress == null || grpcBindAddress.isBlank()) {
            violations.add("GrpcBindAddress must not be null or blank");
        }
        if (kvStoreType == null) {
            violations.add("KvStoreType must not be null");
        }
        if (heartbeatIntervalMs <= 0) {
            violations.add("HeartbeatIntervalMs must be > 0, got: " + heartbeatIntervalMs);
        }
        if (heartbeatMissThreshold < 1) {
            violations.add("HeartbeatMissThreshold must be >= 1, got: " + heartbeatMissThreshold);
        }
        if (heartbeatDisconnectThreshold <= heartbeatMissThreshold) {
            violations.add("HeartbeatDisconnectThreshold (" + heartbeatDisconnectThreshold
                    + ") must be > HeartbeatMissThreshold (" + heartbeatMissThreshold + ")");
        }
        if (heartbeatScanIntervalMs < 100) {
            violations.add("HeartbeatScanIntervalMs must be >= 100, got: " + heartbeatScanIntervalMs);
        }
        if (writeBatchWindowMs <= 0) {
            violations.add("WriteBatchWindowMs must be > 0, got: " + writeBatchWindowMs);
        }
        if (maxJournalLag <= 0) {
            violations.add("MaxJournalLag must be > 0, got: " + maxJournalLag);
        }
        if (maxNodes < 1) {
            violations.add("MaxNodes must be >= 1, got: " + maxNodes);
        }
        if (maxRequestsPerSecondPerNode < 1) {
            violations.add("MaxRequestsPerSecondPerNode must be >= 1, got: " + maxRequestsPerSecondPerNode);
        }
        if (clusterEnabled && (region == null || region.isBlank())) {
            violations.add("Region must not be null or blank when ClusterEnabled is true");
        }
        if (reconnectBurst < 1) {
            violations.add("ReconnectBurst must be >= 1, got: " + reconnectBurst);
        }
        if (reconnectRefillRate < 1) {
            violations.add("ReconnectRefillRate must be >= 1, got: " + reconnectRefillRate);
        }
        if (grpcTlsEnabled) {
            if (grpcTlsCertPath == null || grpcTlsCertPath.isBlank()) {
                violations.add("GrpcTlsCertPath must not be null or blank when GrpcTlsEnabled is true");
            }
            if (grpcTlsKeyPath == null || grpcTlsKeyPath.isBlank()) {
                violations.add("GrpcTlsKeyPath must not be null or blank when GrpcTlsEnabled is true");
            }
        }

        if (!violations.isEmpty()) {
            throw new IllegalArgumentException(
                    "Invalid ControlPlaneConfiguration: " + String.join("; ", violations));
        }

        if (storage != null) {
            storage.validate();
        }

        return this;
    }

    // ---- Setters (for programmatic configuration from Bootstrap) ----

    public ControlPlaneConfiguration grpcPort(int grpcPort) {
        this.grpcPort = grpcPort;
        return this;
    }

    public ControlPlaneConfiguration grpcBindAddress(String grpcBindAddress) {
        this.grpcBindAddress = grpcBindAddress;
        return this;
    }

    public ControlPlaneConfiguration kvStoreType(KvStoreType kvStoreType) {
        this.kvStoreType = kvStoreType;
        return this;
    }

    public ControlPlaneConfiguration heartbeatIntervalMs(long heartbeatIntervalMs) {
        this.heartbeatIntervalMs = heartbeatIntervalMs;
        return this;
    }

    public ControlPlaneConfiguration heartbeatMissThreshold(int heartbeatMissThreshold) {
        this.heartbeatMissThreshold = heartbeatMissThreshold;
        return this;
    }

    public ControlPlaneConfiguration heartbeatDisconnectThreshold(int heartbeatDisconnectThreshold) {
        this.heartbeatDisconnectThreshold = heartbeatDisconnectThreshold;
        return this;
    }

    public ControlPlaneConfiguration heartbeatScanIntervalMs(long heartbeatScanIntervalMs) {
        this.heartbeatScanIntervalMs = heartbeatScanIntervalMs;
        return this;
    }

    public ControlPlaneConfiguration writeBatchWindowMs(long writeBatchWindowMs) {
        this.writeBatchWindowMs = writeBatchWindowMs;
        return this;
    }

    public ControlPlaneConfiguration maxJournalLag(long maxJournalLag) {
        this.maxJournalLag = maxJournalLag;
        return this;
    }

    public ControlPlaneConfiguration maxNodes(int maxNodes) {
        this.maxNodes = maxNodes;
        return this;
    }

    public ControlPlaneConfiguration maxRequestsPerSecondPerNode(int maxRequestsPerSecondPerNode) {
        this.maxRequestsPerSecondPerNode = maxRequestsPerSecondPerNode;
        return this;
    }

    public ControlPlaneConfiguration region(String region) {
        this.region = region;
        return this;
    }

    public ControlPlaneConfiguration clusterEnabled(boolean clusterEnabled) {
        this.clusterEnabled = clusterEnabled;
        return this;
    }

    public ControlPlaneConfiguration reconnectBurst(int reconnectBurst) {
        this.reconnectBurst = reconnectBurst;
        return this;
    }

    public ControlPlaneConfiguration reconnectRefillRate(int reconnectRefillRate) {
        this.reconnectRefillRate = reconnectRefillRate;
        return this;
    }

    public ControlPlaneConfiguration grpcTlsEnabled(boolean grpcTlsEnabled) {
        this.grpcTlsEnabled = grpcTlsEnabled;
        return this;
    }

    public ControlPlaneConfiguration grpcTlsCertPath(String grpcTlsCertPath) {
        this.grpcTlsCertPath = grpcTlsCertPath;
        return this;
    }

    public ControlPlaneConfiguration grpcTlsKeyPath(String grpcTlsKeyPath) {
        this.grpcTlsKeyPath = grpcTlsKeyPath;
        return this;
    }

    public ControlPlaneConfiguration grpcTlsCaPath(String grpcTlsCaPath) {
        this.grpcTlsCaPath = grpcTlsCaPath;
        return this;
    }

    public ControlPlaneConfiguration storage(StorageConfiguration storage) {
        this.storage = storage;
        return this;
    }

    // ---- Getters ----

    public int grpcPort() {
        return grpcPort;
    }

    public String grpcBindAddress() {
        return grpcBindAddress;
    }

    public KvStoreType kvStoreType() {
        return kvStoreType;
    }

    public long heartbeatIntervalMs() {
        return heartbeatIntervalMs;
    }

    public int heartbeatMissThreshold() {
        return heartbeatMissThreshold;
    }

    public int heartbeatDisconnectThreshold() {
        return heartbeatDisconnectThreshold;
    }

    public long heartbeatScanIntervalMs() {
        return heartbeatScanIntervalMs;
    }

    public long writeBatchWindowMs() {
        return writeBatchWindowMs;
    }

    public long maxJournalLag() {
        return maxJournalLag;
    }

    public int maxNodes() {
        return maxNodes;
    }

    public int maxRequestsPerSecondPerNode() {
        return maxRequestsPerSecondPerNode;
    }

    public String region() {
        return region;
    }

    public boolean clusterEnabled() {
        return clusterEnabled;
    }

    public int reconnectBurst() {
        return reconnectBurst;
    }

    public int reconnectRefillRate() {
        return reconnectRefillRate;
    }

    public boolean grpcTlsEnabled() {
        return grpcTlsEnabled;
    }

    public String grpcTlsCertPath() {
        return grpcTlsCertPath;
    }

    public String grpcTlsKeyPath() {
        return grpcTlsKeyPath;
    }

    public String grpcTlsCaPath() {
        return grpcTlsCaPath;
    }

    public StorageConfiguration storage() {
        return storage;
    }

    @Override
    public String toString() {
        return "ControlPlaneConfiguration{" +
                "grpcPort=" + grpcPort +
                ", grpcBindAddress='" + grpcBindAddress + '\'' +
                ", kvStoreType=" + kvStoreType +
                ", heartbeatIntervalMs=" + heartbeatIntervalMs +
                ", heartbeatMissThreshold=" + heartbeatMissThreshold +
                ", heartbeatDisconnectThreshold=" + heartbeatDisconnectThreshold +
                ", heartbeatScanIntervalMs=" + heartbeatScanIntervalMs +
                ", writeBatchWindowMs=" + writeBatchWindowMs +
                ", maxJournalLag=" + maxJournalLag +
                ", maxNodes=" + maxNodes +
                ", maxRequestsPerSecondPerNode=" + maxRequestsPerSecondPerNode +
                ", region='" + region + '\'' +
                ", clusterEnabled=" + clusterEnabled +
                ", reconnectBurst=" + reconnectBurst +
                ", reconnectRefillRate=" + reconnectRefillRate +
                ", grpcTlsEnabled=" + grpcTlsEnabled +
                ", grpcTlsCertPath='" + grpcTlsCertPath + '\'' +
                ", grpcTlsKeyPath='" + grpcTlsKeyPath + '\'' +
                ", grpcTlsCaPath='" + grpcTlsCaPath + '\'' +
                ", storage=" + storage +
                '}';
    }
}
