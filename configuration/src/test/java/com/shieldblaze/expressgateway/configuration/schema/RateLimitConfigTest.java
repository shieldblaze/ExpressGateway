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

class RateLimitConfigTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void validIpBasedRateLimit() {
        var config = new RateLimitConfig("default", 100, 200, "IP", null, 429, 60);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void validHeaderBasedRateLimit() {
        var config = new RateLimitConfig("api-key", 1000, 2000, "HEADER", "X-API-Key", 429, 30);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void validPathBasedRateLimit() {
        var config = new RateLimitConfig("path-limit", 50, 100, "PATH", null, 429, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void zeroRequestsPerSecondRejected() {
        var config = new RateLimitConfig("bad", 0, 100, "IP", null, 429, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void burstSizeLessThanRpsRejected() {
        var config = new RateLimitConfig("bad", 100, 50, "IP", null, 429, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void invalidKeyExtractorRejected() {
        var config = new RateLimitConfig("bad", 100, 200, "COOKIE", null, 429, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void headerExtractorWithoutHeaderNameRejected() {
        var config = new RateLimitConfig("bad", 100, 200, "HEADER", null, 429, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void headerExtractorWithBlankHeaderNameRejected() {
        var config = new RateLimitConfig("bad", 100, 200, "HEADER", "   ", 429, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void invalidStatusCodeRejected() {
        var config = new RateLimitConfig("bad", 100, 200, "IP", null, 99, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void statusCode600Rejected() {
        var config = new RateLimitConfig("bad", 100, 200, "IP", null, 600, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeRetryAfterRejected() {
        var config = new RateLimitConfig("bad", 100, 200, "IP", null, 429, -1);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void zeroRetryAfterAccepted() {
        var config = new RateLimitConfig("ok", 100, 200, "IP", null, 429, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void jsonRoundTrip() throws Exception {
        var original = new RateLimitConfig("api-limit", 1000, 2000, "HEADER", "X-API-Key", 429, 60);
        String json = MAPPER.writeValueAsString(original);
        RateLimitConfig deserialized = MAPPER.readValue(json, RateLimitConfig.class);
        assertEquals(original, deserialized);
    }
}
