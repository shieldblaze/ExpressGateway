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
package com.shieldblaze.expressgateway.controlplane.config.bridge;

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.configuration.http3.Http3Configuration;
import com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.ProxyProtocolMode;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.types.HealthCheckSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.TransportSpec;
import lombok.extern.log4j.Log4j2;

import java.time.Instant;
import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Objects;

/**
 * Bridges between the control plane's {@link ConfigResource} model and the
 * data plane's existing {@link ConfigurationContext} record.
 *
 * <p>The data plane ({@code L4LoadBalancer}, Netty pipeline) uses {@link ConfigurationContext}
 * for transport, TLS, HTTP settings. The control plane uses {@link ConfigResource} for
 * all configuration. This bridge translates between them.</p>
 *
 * <h3>Field mapping semantics</h3>
 * <p>The control plane {@link TransportSpec} is a simplified, API-friendly view of the
 * data plane's {@link TransportConfiguration}. Fields that do not have a direct
 * counterpart in the spec (e.g. {@code tcpConnectionBacklog}, {@code receiveBufferAllocationType})
 * retain their defaults during forward conversion. The reverse direction captures
 * only the fields that exist in the spec, so a round-trip is intentionally lossy
 * for fields the control plane does not yet expose.</p>
 */
@Log4j2
public final class ConfigurationContextBridge {

    private ConfigurationContextBridge() {
        // Static utility class
    }

    // -----------------------------------------------------------------------
    // Forward: ConfigResource collection -> ConfigurationContext
    // -----------------------------------------------------------------------

    /**
     * Build a {@link ConfigurationContext} from resolved {@link ConfigResource}s.
     * Uses defaults for any config types not present in the resources.
     *
     * <p>If multiple resources of the same kind exist, the last one wins.
     * Callers should pre-filter by scope/priority before invoking this method.</p>
     *
     * @param resources the config resources to convert (must not be {@code null})
     * @return a fully validated {@link ConfigurationContext} for the data plane
     * @throws NullPointerException if {@code resources} is {@code null}
     */
    public static ConfigurationContext toConfigurationContext(Collection<ConfigResource> resources) {
        Objects.requireNonNull(resources, "resources");

        BufferConfiguration buffer = BufferConfiguration.DEFAULT;
        EventLoopConfiguration eventLoop = EventLoopConfiguration.DEFAULT;
        EventStreamConfiguration eventStream = EventStreamConfiguration.DEFAULT;
        HealthCheckConfiguration healthCheck = HealthCheckConfiguration.DEFAULT;
        HttpConfiguration http = HttpConfiguration.DEFAULT;
        Http3Configuration http3 = Http3Configuration.DEFAULT;
        QuicConfiguration quic = QuicConfiguration.DEFAULT;
        TlsClientConfiguration tlsClient = TlsClientConfiguration.DEFAULT;
        TlsServerConfiguration tlsServer = TlsServerConfiguration.DEFAULT;
        TransportConfiguration transport = TransportConfiguration.DEFAULT;

        for (ConfigResource resource : resources) {
            String kind = resource.kind().name();
            switch (kind) {
                case "transport" -> {
                    if (resource.spec() instanceof TransportSpec spec) {
                        transport = convertTransport(spec);
                    } else {
                        log.warn("ConfigResource with kind 'transport' has unexpected spec type: {}",
                                resource.spec().getClass().getName());
                    }
                }
                case "health-check" -> {
                    if (resource.spec() instanceof HealthCheckSpec spec) {
                        healthCheck = convertHealthCheck(spec);
                    } else {
                        log.warn("ConfigResource with kind 'health-check' has unexpected spec type: {}",
                                resource.spec().getClass().getName());
                    }
                }
                // Other kinds will be mapped as they gain data plane representations
                // (e.g. "http" -> HttpConfiguration, "buffer" -> BufferConfiguration)
                default -> log.trace("No ConfigurationContext mapping for kind: {}", kind);
            }
        }

        return new ConfigurationContext(buffer, eventLoop, eventStream, healthCheck,
                http, http3, quic, tlsClient, tlsServer, transport);
    }

    // -----------------------------------------------------------------------
    // Reverse: ConfigurationContext -> ConfigResource list
    // -----------------------------------------------------------------------

    /**
     * Convert an existing {@link ConfigurationContext} to a list of {@link ConfigResource}s.
     * Useful for initial migration from file-based config to the control plane model.
     *
     * <p>All generated resources are global-scoped, version 1, and use the
     * provided {@code author} as the creator.</p>
     *
     * @param ctx    the configuration context to convert (must not be {@code null})
     * @param author the principal to record as creator (must not be {@code null})
     * @return a list of {@link ConfigResource}s representing the context
     * @throws NullPointerException if any argument is {@code null}
     */
    public static List<ConfigResource> fromConfigurationContext(ConfigurationContext ctx, String author) {
        Objects.requireNonNull(ctx, "ctx");
        Objects.requireNonNull(author, "author");

        List<ConfigResource> resources = new ArrayList<>();
        Instant now = Instant.now();

        // Transport
        resources.add(new ConfigResource(
                new ConfigResourceId("transport", "global", "default"),
                ConfigKind.TRANSPORT,
                new ConfigScope.Global(),
                1L,
                now,
                now,
                author,
                Map.of(),
                reverseTransport(ctx.transportConfiguration())
        ));

        // Health check
        resources.add(new ConfigResource(
                new ConfigResourceId("health-check", "global", "default"),
                ConfigKind.HEALTH_CHECK,
                new ConfigScope.Global(),
                1L,
                now,
                now,
                author,
                Map.of(),
                reverseHealthCheck(ctx.healthCheckConfiguration())
        ));

        return resources;
    }

    // -----------------------------------------------------------------------
    // Transport: TransportSpec <-> TransportConfiguration
    // -----------------------------------------------------------------------

    /**
     * Convert a {@link TransportSpec} to a validated {@link TransportConfiguration}.
     *
     * <h4>Field mapping</h4>
     * <table>
     *   <tr><th>TransportSpec</th><th>TransportConfiguration</th></tr>
     *   <tr><td>transportType</td><td>transportType (parsed to {@link TransportType} enum)</td></tr>
     *   <tr><td>receiveBufferSize</td><td>socketReceiveBufferSize</td></tr>
     *   <tr><td>sendBufferSize</td><td>socketSendBufferSize</td></tr>
     *   <tr><td>tcpFastOpen</td><td>tcpFastOpenMaximumPendingRequests (100000 if true, 0 if false)</td></tr>
     *   <tr><td>proxyProtocol</td><td>proxyProtocolMode (AUTO if true, OFF if false)</td></tr>
     * </table>
     *
     * <p>Fields not present in the spec ({@code receiveBufferAllocationType},
     * {@code receiveBufferSizes}, {@code tcpConnectionBacklog}, {@code backendConnectTimeout},
     * {@code connectionIdleTimeout}) retain their DEFAULT values.</p>
     */
    private static TransportConfiguration convertTransport(TransportSpec spec) {
        log.debug("Converting TransportSpec '{}' to TransportConfiguration", spec.name());

        TransportType type = parseTransportType(spec.transportType());

        // tcpFastOpen is a boolean in the spec but the data plane uses a pending-request count.
        // When enabled, use the same default as TransportConfiguration.DEFAULT (100_000).
        // When disabled, set to 1 (minimum valid value -- validate() requires positive).
        int tcpFastOpenPending = spec.tcpFastOpen()
                ? TransportConfiguration.DEFAULT.tcpFastOpenMaximumPendingRequests()
                : 1;

        // proxyProtocol is a boolean in the spec but the data plane distinguishes v1/v2/auto.
        // AUTO is the safest choice when the user simply says "enable proxy protocol".
        ProxyProtocolMode proxyMode = spec.proxyProtocol()
                ? ProxyProtocolMode.AUTO
                : ProxyProtocolMode.OFF;

        return new TransportConfiguration()
                .transportType(type)
                .receiveBufferAllocationType(TransportConfiguration.DEFAULT.receiveBufferAllocationType())
                .receiveBufferSizes(TransportConfiguration.DEFAULT.receiveBufferSizes())
                .tcpConnectionBacklog(TransportConfiguration.DEFAULT.tcpConnectionBacklog())
                .socketReceiveBufferSize(spec.receiveBufferSize())
                .socketSendBufferSize(spec.sendBufferSize())
                .tcpFastOpenMaximumPendingRequests(tcpFastOpenPending)
                .backendConnectTimeout(TransportConfiguration.DEFAULT.backendConnectTimeout())
                .connectionIdleTimeout(TransportConfiguration.DEFAULT.connectionIdleTimeout())
                .proxyProtocolMode(proxyMode)
                .validate();
    }

    /**
     * Convert a validated {@link TransportConfiguration} back to a {@link TransportSpec}.
     */
    private static TransportSpec reverseTransport(TransportConfiguration config) {
        return new TransportSpec(
                "default",
                config.transportType().name().toLowerCase(Locale.ROOT),
                config.socketReceiveBufferSize(),
                config.socketSendBufferSize(),
                config.tcpFastOpenMaximumPendingRequests() > 1,
                config.proxyProtocolMode() != ProxyProtocolMode.OFF
        );
    }

    /**
     * Parse a transport type string ("nio", "epoll", "io_uring") to the
     * data plane {@link TransportType} enum. Case-insensitive.
     *
     * @throws IllegalArgumentException if the type string is unrecognized
     */
    private static TransportType parseTransportType(String typeStr) {
        return switch (typeStr.toLowerCase(Locale.ROOT)) {
            case "nio" -> TransportType.NIO;
            case "epoll" -> TransportType.EPOLL;
            case "io_uring" -> TransportType.IO_URING;
            default -> throw new IllegalArgumentException(
                    "Unknown transport type: '" + typeStr + "'. Valid values: nio, epoll, io_uring");
        };
    }

    // -----------------------------------------------------------------------
    // Health check: HealthCheckSpec <-> HealthCheckConfiguration
    // -----------------------------------------------------------------------

    /**
     * Convert a {@link HealthCheckSpec} to a validated {@link HealthCheckConfiguration}.
     *
     * <h4>Field mapping</h4>
     * <table>
     *   <tr><th>HealthCheckSpec</th><th>HealthCheckConfiguration</th></tr>
     *   <tr><td>intervalSeconds</td><td>timeInterval</td></tr>
     *   <tr><td>type, timeoutSeconds, healthyThreshold, unhealthyThreshold, httpPath, expectedStatusCode</td>
     *       <td>No direct mapping -- these are probe-level settings not yet in the data plane config.
     *           Logged at DEBUG level for traceability.</td></tr>
     * </table>
     *
     * <p>The data plane's {@link HealthCheckConfiguration} only exposes {@code workers}
     * and {@code timeInterval}. The richer probe settings from {@link HealthCheckSpec}
     * (thresholds, type, HTTP path) do not have data plane equivalents yet and are
     * noted in log output.</p>
     */
    private static HealthCheckConfiguration convertHealthCheck(HealthCheckSpec spec) {
        log.debug("Converting HealthCheckSpec '{}' to HealthCheckConfiguration", spec.name());

        if (spec.healthyThreshold() != 1 || spec.unhealthyThreshold() != 1) {
            log.info("HealthCheckSpec '{}' defines thresholds (healthy={}, unhealthy={}) " +
                            "which have no data plane equivalent yet -- they will not be applied",
                    spec.name(), spec.healthyThreshold(), spec.unhealthyThreshold());
        }
        if ("http".equals(spec.type())) {
            log.info("HealthCheckSpec '{}' is an HTTP probe (path={}, expectedStatus={}) " +
                            "-- probe type details have no data plane equivalent yet",
                    spec.name(), spec.httpPath(), spec.expectedStatusCode());
        }

        return new HealthCheckConfiguration()
                .workers(HealthCheckConfiguration.DEFAULT.workers())
                .timeInterval(spec.intervalSeconds())
                .validate();
    }

    /**
     * Convert a validated {@link HealthCheckConfiguration} back to a {@link HealthCheckSpec}.
     *
     * <p>Since the data plane config does not carry probe-type information,
     * the reverse direction defaults to a TCP probe with threshold values of 1
     * and no HTTP-specific fields.</p>
     */
    private static HealthCheckSpec reverseHealthCheck(HealthCheckConfiguration config) {
        return new HealthCheckSpec(
                "default",
                "tcp",                  // default probe type -- data plane does not distinguish
                config.timeInterval(),  // intervalSeconds <- timeInterval
                Math.max(1, config.timeInterval() - 1), // timeoutSeconds: must be < intervalSeconds
                1,                      // healthyThreshold: not in data plane, use sensible default
                1,                      // unhealthyThreshold: not in data plane, use sensible default
                null,                   // httpPath: not applicable for TCP probe
                0                       // expectedStatusCode: not applicable for TCP probe
        );
    }
}
