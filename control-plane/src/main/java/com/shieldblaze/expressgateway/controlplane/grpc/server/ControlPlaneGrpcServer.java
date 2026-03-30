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
package com.shieldblaze.expressgateway.controlplane.grpc.server;

import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.grpc.server.interceptor.AuthInterceptor;
import com.shieldblaze.expressgateway.controlplane.grpc.server.interceptor.RateLimitInterceptor;
import io.grpc.Server;
import io.grpc.netty.GrpcSslContexts;
import io.grpc.netty.NettyServerBuilder;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.handler.ssl.ClientAuth;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.util.concurrent.DefaultThreadFactory;
import lombok.extern.log4j.Log4j2;

import javax.net.ssl.SSLException;
import java.io.Closeable;
import java.io.File;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.util.Objects;
import java.util.concurrent.TimeUnit;

/**
 * Netty-based gRPC server for the Control Plane.
 *
 * <p>Runs on dedicated {@link EventLoopGroup}s that are <b>never</b> shared with the
 * data plane. The boss group uses a single thread for TCP accept; the worker group
 * uses {@code max(2, availableProcessors/2)} threads for I/O -- the control plane sees
 * far less traffic than the data path, so half the cores is plenty.</p>
 *
 * <p>TLS support: an optional {@link SslContext} can be provided at construction time.
 * When non-null, the gRPC server is configured with TLS (and optionally mutual TLS
 * when a CA certificate is provided via {@link #createSslContext(String, String, String)}).
 * When null, the server runs in plaintext mode for backward compatibility.</p>
 *
 * <p>Connection lifecycle tuning:</p>
 * <ul>
 *   <li>{@code maxConnectionAge} forces periodic reconnects, which rebalances nodes
 *       across control plane instances behind a DNS or L4 LB.</li>
 *   <li>{@code keepAliveTime / keepAliveTimeout} detect dead TCP connections before the
 *       heartbeat tracker fires. This is important for cloud environments where idle
 *       TCP connections can be silently dropped by NAT gateways.</li>
 *   <li>{@code permitKeepAliveTime} prevents aggressive pinging by buggy clients.</li>
 *   <li>{@code maxInboundMessageSize} is set to 16 MB to accommodate large config
 *       snapshots (e.g., full cluster topology with thousands of backends).</li>
 * </ul>
 *
 * <p>Interceptor ordering: gRPC applies interceptors in reverse registration order
 * (last registered runs first). We register {@code authInterceptor} first and
 * {@code rateLimitInterceptor} second, so the execution order is:
 * rate limit check -> auth check -> service handler. This is intentional: we want
 * to reject rate-limited requests before spending cycles on auth validation.</p>
 */
@Log4j2
public final class ControlPlaneGrpcServer implements Closeable {

    private final int port;
    private final String bindAddress;
    private final Server server;
    private final EventLoopGroup bossGroup;
    private final EventLoopGroup workerGroup;
    private volatile boolean started;

    /**
     * Constructs the gRPC server with all service implementations and interceptors,
     * without TLS (plaintext mode).
     *
     * @param config               the control plane configuration; must not be null
     * @param registrationService  the node registration service; must not be null
     * @param configService        the config distribution service; must not be null
     * @param statsService         the stats collection service; must not be null
     * @param controlService       the node control service; must not be null
     * @param authInterceptor      the authentication interceptor; must not be null
     * @param rateLimitInterceptor the rate limiting interceptor; must not be null
     */
    public ControlPlaneGrpcServer(
            ControlPlaneConfiguration config,
            NodeRegistrationServiceImpl registrationService,
            ConfigDistributionServiceImpl configService,
            StatsCollectionServiceImpl statsService,
            NodeControlServiceImpl controlService,
            AuthInterceptor authInterceptor,
            RateLimitInterceptor rateLimitInterceptor) {
        this(config, registrationService, configService, statsService, controlService,
                authInterceptor, rateLimitInterceptor, null);
    }

    /**
     * Constructs the gRPC server with all service implementations, interceptors,
     * and optional TLS configuration.
     *
     * @param config               the control plane configuration; must not be null
     * @param registrationService  the node registration service; must not be null
     * @param configService        the config distribution service; must not be null
     * @param statsService         the stats collection service; must not be null
     * @param controlService       the node control service; must not be null
     * @param authInterceptor      the authentication interceptor; must not be null
     * @param rateLimitInterceptor the rate limiting interceptor; must not be null
     * @param sslContext           the TLS context for the gRPC server; nullable (null = plaintext)
     */
    public ControlPlaneGrpcServer(
            ControlPlaneConfiguration config,
            NodeRegistrationServiceImpl registrationService,
            ConfigDistributionServiceImpl configService,
            StatsCollectionServiceImpl statsService,
            NodeControlServiceImpl controlService,
            AuthInterceptor authInterceptor,
            RateLimitInterceptor rateLimitInterceptor,
            SslContext sslContext) {

        Objects.requireNonNull(config, "config");
        Objects.requireNonNull(registrationService, "registrationService");
        Objects.requireNonNull(configService, "configService");
        Objects.requireNonNull(statsService, "statsService");
        Objects.requireNonNull(controlService, "controlService");
        Objects.requireNonNull(authInterceptor, "authInterceptor");
        Objects.requireNonNull(rateLimitInterceptor, "rateLimitInterceptor");

        this.port = config.grpcPort();
        this.bindAddress = config.grpcBindAddress();

        // Dedicated EventLoopGroups -- separate from data plane.
        // Boss: 1 thread for accept.
        // Worker: cores/2 threads for I/O (gRPC control plane is less trafficked than data plane).
        int workerThreads = Math.max(2, Runtime.getRuntime().availableProcessors() / 2);
        this.bossGroup = new NioEventLoopGroup(1, new DefaultThreadFactory("cp-grpc-boss"));
        this.workerGroup = new NioEventLoopGroup(workerThreads, new DefaultThreadFactory("cp-grpc-worker"));

        // Build the gRPC server using NettyServerBuilder.
        // Interceptor registration order matters: gRPC executes them in reverse order,
        // so register auth first, rate limiter second -> execution is rate limit -> auth -> handler.
        NettyServerBuilder builder = NettyServerBuilder.forAddress(new InetSocketAddress(bindAddress, port))
                .bossEventLoopGroup(bossGroup)
                .workerEventLoopGroup(workerGroup)
                .channelType(NioServerSocketChannel.class)
                .addService(registrationService)
                .addService(configService)
                .addService(statsService)
                .addService(controlService)
                .intercept(authInterceptor)
                .intercept(rateLimitInterceptor)
                .maxConnectionAge(30, TimeUnit.MINUTES)
                .maxConnectionAgeGrace(5, TimeUnit.MINUTES)
                .keepAliveTime(30, TimeUnit.SECONDS)
                .keepAliveTimeout(10, TimeUnit.SECONDS)
                .permitKeepAliveTime(10, TimeUnit.SECONDS)
                .permitKeepAliveWithoutCalls(true)
                .maxInboundMessageSize(16 * 1024 * 1024); // 16 MB

        if (sslContext != null) {
            builder.sslContext(sslContext);
            log.info("gRPC server TLS enabled");
        } else {
            log.info("gRPC server running in plaintext mode (no TLS)");
        }

        this.server = builder.build();
    }

    /**
     * Creates an {@link SslContext} for the gRPC server using the provided certificate,
     * private key, and optional CA certificate for mutual TLS.
     *
     * <p>When {@code caPath} is non-null and non-empty, mutual TLS (mTLS) is enabled:
     * the server will require clients to present a valid certificate signed by the
     * specified CA. This is the recommended configuration for production deployments
     * where the control plane must authenticate data-plane nodes at the transport layer.</p>
     *
     * @param certPath the path to the server certificate chain (PEM format); must not be null
     * @param keyPath  the path to the server private key (PEM format); must not be null
     * @param caPath   the path to the CA certificate for client auth (PEM format);
     *                 nullable -- if null or empty, client auth is not required (server-only TLS)
     * @return a configured {@link SslContext} suitable for passing to the constructor
     * @throws SSLException if the SSL context cannot be built (bad certs, missing files, etc.)
     */
    public static SslContext createSslContext(String certPath, String keyPath, String caPath) throws SSLException {
        Objects.requireNonNull(certPath, "certPath");
        Objects.requireNonNull(keyPath, "keyPath");

        File certFile = new File(certPath);
        File keyFile = new File(keyPath);

        SslContextBuilder sslBuilder = SslContextBuilder.forServer(certFile, keyFile);
        GrpcSslContexts.configure(sslBuilder);

        if (caPath != null && !caPath.isEmpty()) {
            File caFile = new File(caPath);
            sslBuilder.trustManager(caFile);
            sslBuilder.clientAuth(ClientAuth.REQUIRE);
            log.info("mTLS enabled: client certificates required (CA={})", caPath);
        }

        return sslBuilder.build();
    }

    /**
     * Starts the gRPC server. After this call returns, the server is accepting
     * connections on the configured bind address and port.
     *
     * @throws IOException if the server fails to bind
     */
    public void start() throws IOException {
        server.start();
        started = true;
        log.info("Control Plane gRPC server started on {}:{}", bindAddress, port);
    }

    /**
     * Gracefully shuts down the gRPC server and its EventLoopGroups.
     *
     * <p>Shutdown sequence:</p>
     * <ol>
     *   <li>Initiate graceful server shutdown (stops accepting new RPCs, waits for in-flight)</li>
     *   <li>Wait up to 30 seconds for in-flight RPCs to complete</li>
     *   <li>Force-terminate if the graceful window expires</li>
     *   <li>Shut down boss and worker EventLoopGroups</li>
     * </ol>
     */
    @Override
    public void close() {
        log.info("Shutting down Control Plane gRPC server");
        server.shutdown();
        try {
            if (!server.awaitTermination(30, TimeUnit.SECONDS)) {
                log.warn("gRPC server did not terminate within 30 seconds, forcing shutdown");
                server.shutdownNow();
            }
        } catch (InterruptedException e) {
            server.shutdownNow();
            Thread.currentThread().interrupt();
        }
        try {
            bossGroup.shutdownGracefully().await(5, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            log.warn("Interrupted while awaiting boss EventLoopGroup shutdown");
        }
        try {
            workerGroup.shutdownGracefully().await(5, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            log.warn("Interrupted while awaiting worker EventLoopGroup shutdown");
        }
        log.info("Control Plane gRPC server shut down");
    }

    /**
     * Returns the port the server is actually listening on.
     * Useful when the configured port is 0 (ephemeral port for testing).
     *
     * @return the listening port, or -1 if the server has not started
     */
    public int getPort() {
        return server.getPort();
    }

    /**
     * Returns whether the server is currently accepting RPCs.
     *
     * @return {@code true} if the server has been started and not yet shut down
     */
    public boolean isRunning() {
        return started && !server.isShutdown();
    }
}
