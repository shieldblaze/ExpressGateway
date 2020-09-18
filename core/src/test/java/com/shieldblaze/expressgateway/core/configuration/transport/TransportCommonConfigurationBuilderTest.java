package com.shieldblaze.expressgateway.core.configuration.transport;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertThrows;

class TransportCommonConfigurationBuilderTest {

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
