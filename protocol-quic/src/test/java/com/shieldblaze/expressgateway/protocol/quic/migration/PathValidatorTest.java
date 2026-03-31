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
package com.shieldblaze.expressgateway.protocol.quic.migration;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link PathValidator} -- PATH_CHALLENGE/PATH_RESPONSE validation
 * per RFC 9000 Section 8.2.
 */
@Timeout(10)
class PathValidatorTest {

    private static final InetSocketAddress ADDR_1 = new InetSocketAddress("192.168.1.100", 12345);
    private static final InetSocketAddress ADDR_2 = new InetSocketAddress("10.0.0.50", 54321);

    @Test
    void initiateAndValidate_roundTrip_succeeds() {
        PathValidator validator = new PathValidator();
        byte[] challenge = validator.initiateChallenge(ADDR_1);

        assertNotNull(challenge);
        assertEquals(8, challenge.length, "PATH_CHALLENGE data must be 8 bytes per RFC 9000 Section 19.17");

        PathValidator.ValidationResult result = validator.validateResponse(ADDR_1, challenge);
        assertEquals(PathValidator.ValidationResult.VALIDATED, result);
        assertEquals(1, validator.challengesValidated());
    }

    @Test
    void validateResponse_wrongData_returnsNoMatch() {
        PathValidator validator = new PathValidator();
        validator.initiateChallenge(ADDR_1);

        byte[] wrongData = {0, 0, 0, 0, 0, 0, 0, 0};
        PathValidator.ValidationResult result = validator.validateResponse(ADDR_1, wrongData);
        assertEquals(PathValidator.ValidationResult.NO_MATCH, result);
        assertEquals(1, validator.challengesFailed());
    }

    @Test
    void validateResponse_unknownAddress_returnsNoMatch() {
        PathValidator validator = new PathValidator();
        validator.initiateChallenge(ADDR_1);

        byte[] challenge = new byte[8]; // Wrong challenge data doesn't matter -- address won't match
        PathValidator.ValidationResult result = validator.validateResponse(ADDR_2, challenge);
        assertEquals(PathValidator.ValidationResult.NO_MATCH, result);
    }

    @Test
    void validateResponse_expiredChallenge_returnsExpired() throws Exception {
        // Use very short timeout for testing
        PathValidator validator = new PathValidator(1_000_000L); // 1ms timeout
        byte[] challenge = validator.initiateChallenge(ADDR_1);

        Thread.sleep(50); // Wait for timeout

        PathValidator.ValidationResult result = validator.validateResponse(ADDR_1, challenge);
        assertEquals(PathValidator.ValidationResult.EXPIRED, result);
    }

    @Test
    void hasPendingChallenge_tracksState() {
        PathValidator validator = new PathValidator();
        assertFalse(validator.hasPendingChallenge(ADDR_1));

        validator.initiateChallenge(ADDR_1);
        assertTrue(validator.hasPendingChallenge(ADDR_1));

        // After validation, the pending challenge is consumed
        byte[] challenge = validator.initiateChallenge(ADDR_1);
        validator.validateResponse(ADDR_1, challenge);
        assertFalse(validator.hasPendingChallenge(ADDR_1));
    }

    @Test
    void cancelChallenge_removesPending() {
        PathValidator validator = new PathValidator();
        validator.initiateChallenge(ADDR_1);
        assertTrue(validator.hasPendingChallenge(ADDR_1));

        validator.cancelChallenge(ADDR_1);
        assertFalse(validator.hasPendingChallenge(ADDR_1));
    }

    @Test
    void evictExpired_removesTimedOutChallenges() throws Exception {
        PathValidator validator = new PathValidator(1_000_000L); // 1ms
        validator.initiateChallenge(ADDR_1);
        validator.initiateChallenge(ADDR_2);
        assertEquals(2, validator.pendingCount());

        Thread.sleep(50);
        int evicted = validator.evictExpired();
        assertEquals(2, evicted);
        assertEquals(0, validator.pendingCount());
    }

    @Test
    void nullResponseData_returnsNoMatch() {
        PathValidator validator = new PathValidator();
        validator.initiateChallenge(ADDR_1);

        assertEquals(PathValidator.ValidationResult.NO_MATCH,
                validator.validateResponse(ADDR_1, null));
    }

    @Test
    void wrongLengthResponseData_returnsNoMatch() {
        PathValidator validator = new PathValidator();
        validator.initiateChallenge(ADDR_1);

        assertEquals(PathValidator.ValidationResult.NO_MATCH,
                validator.validateResponse(ADDR_1, new byte[7]));
        // Note: challenge was consumed on first call, second call for same addr has no pending
    }

    @Test
    void multipleAddresses_independentChallenges() {
        PathValidator validator = new PathValidator();
        byte[] challenge1 = validator.initiateChallenge(ADDR_1);
        byte[] challenge2 = validator.initiateChallenge(ADDR_2);

        // Validate addr2 first
        assertEquals(PathValidator.ValidationResult.VALIDATED,
                validator.validateResponse(ADDR_2, challenge2));
        // addr1 still pending
        assertTrue(validator.hasPendingChallenge(ADDR_1));
        assertEquals(PathValidator.ValidationResult.VALIDATED,
                validator.validateResponse(ADDR_1, challenge1));
    }

    @Test
    void metrics_tracked() {
        PathValidator validator = new PathValidator();
        byte[] c1 = validator.initiateChallenge(ADDR_1);
        validator.initiateChallenge(ADDR_2);

        validator.validateResponse(ADDR_1, c1); // success
        validator.validateResponse(ADDR_2, new byte[8]); // fail

        assertEquals(2, validator.challengesSent());
        assertEquals(1, validator.challengesValidated());
        assertEquals(1, validator.challengesFailed());
    }
}
