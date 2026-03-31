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
package com.shieldblaze.expressgateway.controlplane.kvstore;

import com.fasterxml.jackson.annotation.JsonProperty;
import lombok.extern.log4j.Log4j2;

import java.util.ArrayList;
import java.util.List;

/**
 * Configuration for KV store backend connection settings.
 *
 * <p>Supports connection, TLS, authentication, and health check settings
 * for all supported backends (ZooKeeper, etcd, Consul). Backend-specific
 * fields are ignored when using a different backend.</p>
 *
 * <p>After construction or deserialization, call {@link #validate()} to verify
 * that all values are within acceptable ranges. Invalid configurations throw
 * {@link IllegalArgumentException} listing every violation.</p>
 */
@Log4j2
public final class StorageConfiguration {

    // ---- Connection settings ----

    @JsonProperty("Endpoints")
    private List<String> endpoints = new ArrayList<>();

    // ---- TLS settings ----

    @JsonProperty("TlsEnabled")
    private boolean tlsEnabled;

    @JsonProperty("TlsCertPath")
    private String tlsCertPath;

    @JsonProperty("TlsKeyPath")
    private String tlsKeyPath;

    @JsonProperty("TlsCaPath")
    private String tlsCaPath;

    // ---- Timeouts ----

    @JsonProperty("ConnectTimeoutMs")
    private long connectTimeoutMs = 5000;

    @JsonProperty("OperationTimeoutMs")
    private long operationTimeoutMs = 10000;

    // ---- Authentication ----

    @JsonProperty("Username")
    private String username;

    @JsonProperty("Password")
    private String password;

    // ---- Consul-specific ----

    @JsonProperty("ConsulToken")
    private String consulToken;

    @JsonProperty("ConsulDatacenter")
    private String consulDatacenter;

    // ---- ZooKeeper-specific ----

    @JsonProperty("ZkSessionTimeoutMs")
    private int zkSessionTimeoutMs = 30000;

    @JsonProperty("ZkNamespace")
    private String zkNamespace;

    // ---- etcd-specific ----

    @JsonProperty("EtcdNamespace")
    private String etcdNamespace;

    // ---- Health check ----

    @JsonProperty("HealthCheckIntervalMs")
    private long healthCheckIntervalMs = 10000;

    @JsonProperty("StartupHealthCheckTimeoutMs")
    private long startupHealthCheckTimeoutMs = 30000;

    @JsonProperty("StartupHealthCheckRetries")
    private int startupHealthCheckRetries = 3;

    @JsonProperty("MaxAcceptableLatencyMs")
    private long maxAcceptableLatencyMs = 500;

    /**
     * Default constructor for Jackson deserialization.
     * All fields are initialized to their defaults.
     */
    public StorageConfiguration() {
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
    public StorageConfiguration validate() {
        List<String> violations = new ArrayList<>();

        if (endpoints == null || endpoints.isEmpty()) {
            violations.add("Endpoints must not be null or empty");
        } else {
            for (int i = 0; i < endpoints.size(); i++) {
                String ep = endpoints.get(i);
                if (ep == null || ep.isBlank()) {
                    violations.add("Endpoints[" + i + "] must not be null or blank");
                }
            }
        }

        if (connectTimeoutMs <= 0) {
            violations.add("ConnectTimeoutMs must be > 0, got: " + connectTimeoutMs);
        }
        if (operationTimeoutMs <= 0) {
            violations.add("OperationTimeoutMs must be > 0, got: " + operationTimeoutMs);
        }

        if (tlsEnabled) {
            if (tlsCaPath == null || tlsCaPath.isBlank()) {
                violations.add("TlsCaPath must not be null or blank when TLS is enabled");
            }
        }

        if (zkSessionTimeoutMs < 1000) {
            violations.add("ZkSessionTimeoutMs must be >= 1000, got: " + zkSessionTimeoutMs);
        }

        if (healthCheckIntervalMs <= 0) {
            violations.add("HealthCheckIntervalMs must be > 0, got: " + healthCheckIntervalMs);
        }
        if (startupHealthCheckTimeoutMs <= 0) {
            violations.add("StartupHealthCheckTimeoutMs must be > 0, got: " + startupHealthCheckTimeoutMs);
        }
        if (startupHealthCheckRetries < 1) {
            violations.add("StartupHealthCheckRetries must be >= 1, got: " + startupHealthCheckRetries);
        }
        if (maxAcceptableLatencyMs <= 0) {
            violations.add("MaxAcceptableLatencyMs must be > 0, got: " + maxAcceptableLatencyMs);
        }

        if (!violations.isEmpty()) {
            throw new IllegalArgumentException(
                    "Invalid StorageConfiguration: " + String.join("; ", violations));
        }

        return this;
    }

    // ---- Fluent setters ----

    public StorageConfiguration endpoints(List<String> endpoints) {
        this.endpoints = endpoints;
        return this;
    }

    public StorageConfiguration tlsEnabled(boolean tlsEnabled) {
        this.tlsEnabled = tlsEnabled;
        return this;
    }

    public StorageConfiguration tlsCertPath(String tlsCertPath) {
        this.tlsCertPath = tlsCertPath;
        return this;
    }

    public StorageConfiguration tlsKeyPath(String tlsKeyPath) {
        this.tlsKeyPath = tlsKeyPath;
        return this;
    }

    public StorageConfiguration tlsCaPath(String tlsCaPath) {
        this.tlsCaPath = tlsCaPath;
        return this;
    }

    public StorageConfiguration connectTimeoutMs(long connectTimeoutMs) {
        this.connectTimeoutMs = connectTimeoutMs;
        return this;
    }

    public StorageConfiguration operationTimeoutMs(long operationTimeoutMs) {
        this.operationTimeoutMs = operationTimeoutMs;
        return this;
    }

    public StorageConfiguration username(String username) {
        this.username = username;
        return this;
    }

    public StorageConfiguration password(String password) {
        this.password = password;
        return this;
    }

    public StorageConfiguration consulToken(String consulToken) {
        this.consulToken = consulToken;
        return this;
    }

    public StorageConfiguration consulDatacenter(String consulDatacenter) {
        this.consulDatacenter = consulDatacenter;
        return this;
    }

    public StorageConfiguration zkSessionTimeoutMs(int zkSessionTimeoutMs) {
        this.zkSessionTimeoutMs = zkSessionTimeoutMs;
        return this;
    }

    public StorageConfiguration zkNamespace(String zkNamespace) {
        this.zkNamespace = zkNamespace;
        return this;
    }

    public StorageConfiguration etcdNamespace(String etcdNamespace) {
        this.etcdNamespace = etcdNamespace;
        return this;
    }

    public StorageConfiguration healthCheckIntervalMs(long healthCheckIntervalMs) {
        this.healthCheckIntervalMs = healthCheckIntervalMs;
        return this;
    }

    public StorageConfiguration startupHealthCheckTimeoutMs(long startupHealthCheckTimeoutMs) {
        this.startupHealthCheckTimeoutMs = startupHealthCheckTimeoutMs;
        return this;
    }

    public StorageConfiguration startupHealthCheckRetries(int startupHealthCheckRetries) {
        this.startupHealthCheckRetries = startupHealthCheckRetries;
        return this;
    }

    public StorageConfiguration maxAcceptableLatencyMs(long maxAcceptableLatencyMs) {
        this.maxAcceptableLatencyMs = maxAcceptableLatencyMs;
        return this;
    }

    // ---- Getters ----

    public List<String> endpoints() {
        return endpoints;
    }

    public boolean tlsEnabled() {
        return tlsEnabled;
    }

    public String tlsCertPath() {
        return tlsCertPath;
    }

    public String tlsKeyPath() {
        return tlsKeyPath;
    }

    public String tlsCaPath() {
        return tlsCaPath;
    }

    public long connectTimeoutMs() {
        return connectTimeoutMs;
    }

    public long operationTimeoutMs() {
        return operationTimeoutMs;
    }

    public String username() {
        return username;
    }

    public String password() {
        return password;
    }

    public String consulToken() {
        return consulToken;
    }

    public String consulDatacenter() {
        return consulDatacenter;
    }

    public int zkSessionTimeoutMs() {
        return zkSessionTimeoutMs;
    }

    public String zkNamespace() {
        return zkNamespace;
    }

    public String etcdNamespace() {
        return etcdNamespace;
    }

    public long healthCheckIntervalMs() {
        return healthCheckIntervalMs;
    }

    public long startupHealthCheckTimeoutMs() {
        return startupHealthCheckTimeoutMs;
    }

    public int startupHealthCheckRetries() {
        return startupHealthCheckRetries;
    }

    public long maxAcceptableLatencyMs() {
        return maxAcceptableLatencyMs;
    }

    @Override
    public String toString() {
        return "StorageConfiguration{" +
                "endpoints=" + endpoints +
                ", tlsEnabled=" + tlsEnabled +
                ", connectTimeoutMs=" + connectTimeoutMs +
                ", operationTimeoutMs=" + operationTimeoutMs +
                ", zkSessionTimeoutMs=" + zkSessionTimeoutMs +
                ", healthCheckIntervalMs=" + healthCheckIntervalMs +
                ", startupHealthCheckTimeoutMs=" + startupHealthCheckTimeoutMs +
                ", startupHealthCheckRetries=" + startupHealthCheckRetries +
                ", maxAcceptableLatencyMs=" + maxAcceptableLatencyMs +
                '}';
    }
}
