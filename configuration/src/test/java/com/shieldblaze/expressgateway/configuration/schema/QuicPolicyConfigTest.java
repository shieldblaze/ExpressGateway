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
package com.shieldblaze.expressgateway.configuration.schema;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class QuicPolicyConfigTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void validQuicPolicy() {
        var config = new QuicPolicyConfig(30000, 1350, 10_000_000, 1_000_000, 1_000_000,
                1_000_000, 100, 3, 3, 25, false, 8);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void minUdpPayloadSizeOf1200Accepted() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, 0, 0, 0, 0, true, 2);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void udpPayloadSizeBelow1200Rejected() {
        var config = new QuicPolicyConfig(0, 1199, 0, 0, 0, 0, 0, 0, 0, 0, true, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void maxUdpPayloadSizeOf65527Accepted() {
        var config = new QuicPolicyConfig(0, 65527, 1_000_000, 0, 0, 0, 0, 0, 0, 0, true, 2);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void maxUdpPayloadSizeAbove65527Rejected() {
        var config = new QuicPolicyConfig(0, 65528, 1_000_000, 0, 0, 0, 0, 0, 0, 0, true, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void initialMaxDataZeroAcceptedButWarns() {
        // initialMaxData == 0 should be accepted (it is valid per RFC) but logs a warning
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, 0, 0, 0, 0, true, 2);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void negativeMaxIdleTimeoutRejected() {
        var config = new QuicPolicyConfig(-1, 1200, 0, 0, 0, 0, 0, 0, 0, 0, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeInitialMaxDataRejected() {
        var config = new QuicPolicyConfig(0, 1200, -1, 0, 0, 0, 0, 0, 0, 0, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeStreamDataBidiLocalRejected() {
        var config = new QuicPolicyConfig(0, 1200, 0, -1, 0, 0, 0, 0, 0, 0, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeStreamDataBidiRemoteRejected() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, -1, 0, 0, 0, 0, 0, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeStreamDataUniRejected() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, -1, 0, 0, 0, 0, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeStreamsBidiRejected() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, -1, 0, 0, 0, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeStreamsUniRejected() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, 0, -1, 0, 0, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void ackDelayExponentAbove20Rejected() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, 0, 0, 21, 0, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeAckDelayExponentRejected() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, 0, 0, -1, 0, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void maxAckDelayAt16384Rejected() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, 0, 0, 0, 16384, false, 2);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void maxAckDelayAt16383Accepted() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, 0, 0, 0, 16383, false, 2);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void activeConnectionIdLimitBelow2Rejected() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, 0, 0, 0, 0, false, 1);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void activeConnectionIdLimitOf2Accepted() {
        var config = new QuicPolicyConfig(0, 1200, 0, 0, 0, 0, 0, 0, 0, 0, false, 2);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void disableActiveMigrationAccepted() {
        var config = new QuicPolicyConfig(30000, 1350, 10_000_000, 1_000_000, 1_000_000,
                1_000_000, 100, 3, 3, 25, true, 2);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void jsonRoundTrip() throws Exception {
        var original = new QuicPolicyConfig(30000, 1350, 10_000_000, 1_000_000, 1_000_000,
                1_000_000, 100, 3, 3, 25, false, 8);
        String json = MAPPER.writeValueAsString(original);
        QuicPolicyConfig deserialized = MAPPER.readValue(json, QuicPolicyConfig.class);
        assertEquals(original, deserialized);
    }
}
