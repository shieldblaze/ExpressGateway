/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.configuration.transport;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertThrows;

final class TransportConfigurationBuilderTest {

    @Test
    void throwsException() {

        // TransportType is `null`
        assertThrows(NullPointerException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(null)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .withReceiveBufferSizes(new int[]{1, 2, 3, 4})
                .build());

        // withReceiveBufferAllocationType is `null`
        assertThrows(NullPointerException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(null)
                .withReceiveBufferSizes(new int[]{1, 2, 3, 4})
                .build());

        // Receive Sizes Array is `null`
        assertThrows(NullPointerException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(null)
                .build());

        // Backlog less than 0
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .withReceiveBufferSizes(new int[]{100,200,300})
                .withTCPConnectionBacklog(0)
                .build());

        // Invalid Buffers
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .withReceiveBufferSizes(new int[]{1, 2, 3, 4})
                .build());

        // Max buffer higher than 65536
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .withReceiveBufferSizes(new int[]{64, 1500, 70000})
                .build());

        // Max buffer lower than 64
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .withReceiveBufferSizes(new int[]{64, 1500, 63})
                .build());

        // Initial buffer lower than Minimum Buffer
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .withReceiveBufferSizes(new int[]{64, 63, 65535})
                .build());

        // Initial buffer lower than Minimum Buffer
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .withReceiveBufferSizes(new int[]{100, 63, 65535})
                .build());

        // Maximum buffer lower than Minimum Buffer
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .withReceiveBufferSizes(new int[]{100, 100, 80})
                .build());

        // Fixed buffer lower than 64
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{63})
                .build());

        // Fixed buffer more than 65537
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{65537})
                .build());

        // Fixed buffer is invalid
        assertThrows(IllegalArgumentException.class, () -> TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{100,200,300})
                .build());
    }
}
