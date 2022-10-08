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

import static com.shieldblaze.expressgateway.common.utils.StringUtil.EMPTY_STRING;
import static com.shieldblaze.expressgateway.common.utils.StringUtil.checkNullOrEmpty;
import static com.shieldblaze.expressgateway.common.utils.StringUtil.validateNotNullOrEmpty;
import static org.junit.jupiter.api.Assertions.*;

class StringUtilTest {

    @Test
    void checkNullOrEmptyTest() {
        assertTrue(checkNullOrEmpty(EMPTY_STRING));
        assertTrue(checkNullOrEmpty(null));
        assertTrue(checkNullOrEmpty(" "));

        assertFalse(checkNullOrEmpty("Test"));
        assertFalse(checkNullOrEmpty("@"));
        assertFalse(checkNullOrEmpty("."));
    }

    @Test
    void validateNotNullOrEmptyTest() {
        assertThrows(NullPointerException.class,  () -> validateNotNullOrEmpty(null));
        assertThrows(IllegalArgumentException.class,  () -> validateNotNullOrEmpty(EMPTY_STRING));
        assertThrows(IllegalArgumentException.class, () -> validateNotNullOrEmpty(" "));

        assertDoesNotThrow(() -> validateNotNullOrEmpty("."));
        assertDoesNotThrow(() -> validateNotNullOrEmpty("Test"));
        assertDoesNotThrow(() -> validateNotNullOrEmpty("@"));
    }
}
