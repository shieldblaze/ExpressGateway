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
package com.shieldblaze.expressgateway.common.utils;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

/**
 * Tests for {@link LogSanitizer}.
 */
class LogSanitizerTest {

    @Test
    void sanitizeRemovesCarriageReturn() {
        assertEquals("hello world", LogSanitizer.sanitize("hello\rworld"));
    }

    @Test
    void sanitizeRemovesNewline() {
        assertEquals("hello world", LogSanitizer.sanitize("hello\nworld"));
    }

    @Test
    void sanitizeRemovesCRLF() {
        assertEquals("hello  world", LogSanitizer.sanitize("hello\r\nworld"));
    }

    @Test
    void sanitizeReturnsNullStringForNullInput() {
        assertEquals("null", LogSanitizer.sanitize((String) null));
    }

    @Test
    void sanitizeReturnsNullStringForNullCharSequence() {
        assertEquals("null", LogSanitizer.sanitize((CharSequence) null));
    }

    @Test
    void sanitizePassesThroughCleanString() {
        assertEquals("clean-value", LogSanitizer.sanitize("clean-value"));
    }

    @Test
    void sanitizePassesThroughEmptyString() {
        assertEquals("", LogSanitizer.sanitize(""));
    }

    @Test
    void sanitizeHandlesCharSequence() {
        CharSequence cs = new StringBuilder("line1\nline2");
        assertEquals("line1 line2", LogSanitizer.sanitize(cs));
    }

    @Test
    void sanitizeHandlesLogInjectionAttempt() {
        // A typical CWE-117 attack: injecting a fake log line
        String malicious = "safe-value\n2026-03-29 ERROR Fake log entry injected";
        String sanitized = LogSanitizer.sanitize(malicious);
        // The sanitized string should NOT contain any newline
        assertEquals(-1, sanitized.indexOf('\n'));
        assertEquals(-1, sanitized.indexOf('\r'));
        assertEquals("safe-value 2026-03-29 ERROR Fake log entry injected", sanitized);
    }
}
