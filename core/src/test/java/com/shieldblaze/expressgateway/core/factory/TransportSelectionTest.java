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
package com.shieldblaze.expressgateway.core.factory;

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.incubator.channel.uring.IOUring;
import io.netty.incubator.channel.uring.IOUringEventLoopGroup;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import org.junit.jupiter.api.condition.EnabledIf;

import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * V-TEST-060: Transport selection tests.
 *
 * <p>Validates that {@link EventLoopFactory} creates the correct {@link EventLoopGroup}
 * implementation based on the configured {@link TransportType}:
 * <ul>
 *   <li>{@code NIO} -- {@link NioEventLoopGroup} (always available)</li>
 *   <li>{@code EPOLL} -- {@link EpollEventLoopGroup} (Linux only)</li>
 *   <li>{@code IO_URING} -- {@link IOUringEventLoopGroup} (Linux 5.1+ only)</li>
 * </ul>
 *
 * <p>The EPOLL and IO_URING tests are gated by runtime availability checks using
 * JUnit's {@code @EnabledIf} to avoid failures on non-Linux platforms or kernels
 * that lack io_uring support.</p>
 */
@Timeout(value = 30)
class TransportSelectionTest {

    // ===================================================================
    // NIO transport (always available)
    // ===================================================================

    /**
     * NIO is the universal Java transport and must always work.
     * Verifies both parent and child groups are NioEventLoopGroup instances.
     */
    @Test
    void nioTransport_createsNioEventLoopGroups() throws Exception {
        TransportConfiguration transportConfig = new TransportConfiguration()
                .setTransportType(TransportType.NIO)
                .setReceiveBufferAllocationType(
                        com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType.ADAPTIVE)
                .setReceiveBufferSizes(new int[]{512, 9001, 65535})
                .setTcpConnectionBacklog(1024)
                .setSocketReceiveBufferSize(262144)
                .setSocketSendBufferSize(262144)
                .setTcpFastOpenMaximumPendingRequests(100)
                .setBackendConnectTimeout(10000)
                .setConnectionIdleTimeout(120000)
                .validate();

        EventLoopConfiguration elConfig = new EventLoopConfiguration()
                .setParentWorkers(1)
                .setChildWorkers(2)
                .validate();

        ConfigurationContext ctx = new ConfigurationContext(
                BufferConfiguration.DEFAULT,
                elConfig,
                EventStreamConfiguration.DEFAULT,
                HealthCheckConfiguration.DEFAULT,
                HttpConfiguration.DEFAULT,
                com.shieldblaze.expressgateway.configuration.http3.Http3Configuration.DEFAULT,
                com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration.DEFAULT,
                TlsClientConfiguration.DEFAULT,
                TlsServerConfiguration.DEFAULT,
                transportConfig
        );

        EventLoopFactory factory = new EventLoopFactory(ctx);
        try {
            assertNotNull(factory.parentGroup(), "Parent group must not be null");
            assertNotNull(factory.childGroup(), "Child group must not be null");

            assertInstanceOf(NioEventLoopGroup.class, factory.parentGroup(),
                    "NIO transport must create NioEventLoopGroup for parent");
            assertInstanceOf(NioEventLoopGroup.class, factory.childGroup(),
                    "NIO transport must create NioEventLoopGroup for child");
        } finally {
            shutdownGroups(factory);
        }
    }

    /**
     * Verifies that the parent and child groups have the correct number of
     * threads as specified in EventLoopConfiguration.
     */
    @Test
    void nioTransport_respectsWorkerCounts() throws Exception {
        int parentWorkers = 2;
        int childWorkers = 4;

        EventLoopConfiguration elConfig = new EventLoopConfiguration()
                .setParentWorkers(parentWorkers)
                .setChildWorkers(childWorkers)
                .validate();

        ConfigurationContext ctx = buildConfigContext(TransportType.NIO, elConfig);
        EventLoopFactory factory = new EventLoopFactory(ctx);
        try {
            // NioEventLoopGroup.executorCount() returns the number of EventLoop threads
            NioEventLoopGroup parent = (NioEventLoopGroup) factory.parentGroup();
            NioEventLoopGroup child = (NioEventLoopGroup) factory.childGroup();

            assertEquals(parentWorkers, parent.executorCount(),
                    "Parent group must have " + parentWorkers + " workers");
            assertEquals(childWorkers, child.executorCount(),
                    "Child group must have " + childWorkers + " workers");
        } finally {
            shutdownGroups(factory);
        }
    }

    // ===================================================================
    // EPOLL transport (Linux only)
    // ===================================================================

    /**
     * EPOLL transport should create EpollEventLoopGroup instances.
     * Only runs on systems where Epoll is available (Linux with native transport).
     */
    @Test
    @EnabledIf("isEpollAvailable")
    void epollTransport_createsEpollEventLoopGroups() throws Exception {
        EventLoopConfiguration elConfig = new EventLoopConfiguration()
                .setParentWorkers(1)
                .setChildWorkers(2)
                .validate();

        ConfigurationContext ctx = buildConfigContext(TransportType.EPOLL, elConfig);
        EventLoopFactory factory = new EventLoopFactory(ctx);
        try {
            assertInstanceOf(EpollEventLoopGroup.class, factory.parentGroup(),
                    "EPOLL transport must create EpollEventLoopGroup for parent");
            assertInstanceOf(EpollEventLoopGroup.class, factory.childGroup(),
                    "EPOLL transport must create EpollEventLoopGroup for child");
        } finally {
            shutdownGroups(factory);
        }
    }

    // ===================================================================
    // IO_URING transport (Linux 5.1+ only)
    // ===================================================================

    /**
     * IO_URING transport should create IOUringEventLoopGroup instances.
     * Only runs on systems where io_uring is available (Linux 5.1+ with native lib).
     */
    @Test
    @EnabledIf("isIOUringAvailable")
    void ioUringTransport_createsIOUringEventLoopGroups() throws Exception {
        EventLoopConfiguration elConfig = new EventLoopConfiguration()
                .setParentWorkers(1)
                .setChildWorkers(2)
                .validate();

        ConfigurationContext ctx = buildConfigContext(TransportType.IO_URING, elConfig);
        EventLoopFactory factory = new EventLoopFactory(ctx);
        try {
            assertInstanceOf(IOUringEventLoopGroup.class, factory.parentGroup(),
                    "IO_URING transport must create IOUringEventLoopGroup for parent");
            assertInstanceOf(IOUringEventLoopGroup.class, factory.childGroup(),
                    "IO_URING transport must create IOUringEventLoopGroup for child");
        } finally {
            shutdownGroups(factory);
        }
    }

    // ===================================================================
    // Default transport selection
    // ===================================================================

    /**
     * The DEFAULT TransportConfiguration should auto-detect the best available
     * transport: IO_URING > EPOLL > NIO. This test verifies the auto-detection
     * produces a valid, functional EventLoopFactory.
     */
    @Test
    void defaultTransport_autoDetectsAndCreatesValidGroups() throws Exception {
        ConfigurationContext ctx = ConfigurationContext.DEFAULT;
        EventLoopFactory factory = new EventLoopFactory(ctx);
        try {
            assertNotNull(factory.parentGroup(), "Default parent group must not be null");
            assertNotNull(factory.childGroup(), "Default child group must not be null");

            TransportType expectedType = TransportConfiguration.DEFAULT.transportType();
            EventLoopGroup parentGroup = factory.parentGroup();

            switch (expectedType) {
                case IO_URING -> assertInstanceOf(IOUringEventLoopGroup.class, parentGroup,
                        "IO_URING detected but parent group is not IOUringEventLoopGroup");
                case EPOLL -> assertInstanceOf(EpollEventLoopGroup.class, parentGroup,
                        "EPOLL detected but parent group is not EpollEventLoopGroup");
                case NIO -> assertInstanceOf(NioEventLoopGroup.class, parentGroup,
                        "NIO detected but parent group is not NioEventLoopGroup");
            }
        } finally {
            shutdownGroups(factory);
        }
    }

    /**
     * Verify that the transport type detection in TransportConfiguration.DEFAULT
     * follows the priority: IO_URING > EPOLL > NIO.
     */
    @Test
    void defaultTransportType_followsPriorityOrder() {
        TransportType defaultType = TransportConfiguration.DEFAULT.transportType();

        if (IOUring.isAvailable()) {
            assertEquals(TransportType.IO_URING, defaultType,
                    "When io_uring is available, it must be the default transport");
        } else if (Epoll.isAvailable()) {
            assertEquals(TransportType.EPOLL, defaultType,
                    "When epoll is available (and io_uring is not), it must be the default transport");
        } else {
            assertEquals(TransportType.NIO, defaultType,
                    "When no native transport is available, NIO must be the default");
        }
    }

    /**
     * Verify TransportType.nativeTransport() returns correct values.
     */
    @Test
    void transportType_nativeTransportFlag() {
        assertTrue(!TransportType.NIO.nativeTransport(),
                "NIO must not be a native transport");
        assertTrue(TransportType.EPOLL.nativeTransport(),
                "EPOLL must be a native transport");
        assertTrue(TransportType.IO_URING.nativeTransport(),
                "IO_URING must be a native transport");
    }

    // ===================================================================
    // Helpers
    // ===================================================================

    private ConfigurationContext buildConfigContext(TransportType type, EventLoopConfiguration elConfig) {
        TransportConfiguration transportConfig = new TransportConfiguration()
                .setTransportType(type)
                .setReceiveBufferAllocationType(
                        com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType.ADAPTIVE)
                .setReceiveBufferSizes(new int[]{512, 9001, 65535})
                .setTcpConnectionBacklog(1024)
                .setSocketReceiveBufferSize(262144)
                .setSocketSendBufferSize(262144)
                .setTcpFastOpenMaximumPendingRequests(100)
                .setBackendConnectTimeout(10000)
                .setConnectionIdleTimeout(120000)
                .validate();

        return new ConfigurationContext(
                BufferConfiguration.DEFAULT,
                elConfig,
                EventStreamConfiguration.DEFAULT,
                HealthCheckConfiguration.DEFAULT,
                HttpConfiguration.DEFAULT,
                com.shieldblaze.expressgateway.configuration.http3.Http3Configuration.DEFAULT,
                com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration.DEFAULT,
                TlsClientConfiguration.DEFAULT,
                TlsServerConfiguration.DEFAULT,
                transportConfig
        );
    }

    private void shutdownGroups(EventLoopFactory factory) throws Exception {
        factory.parentGroup().shutdownGracefully(0, 1, TimeUnit.SECONDS).sync();
        factory.childGroup().shutdownGracefully(0, 1, TimeUnit.SECONDS).sync();
    }

    // JUnit @EnabledIf condition methods must be static
    static boolean isEpollAvailable() {
        return Epoll.isAvailable();
    }

    static boolean isIOUringAvailable() {
        return IOUring.isAvailable();
    }
}
