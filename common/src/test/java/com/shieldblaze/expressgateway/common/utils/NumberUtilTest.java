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

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class NumberUtilTest {

    @Test
    void testCheckZeroOrPositive() {
        assertDoesNotThrow(() -> NumberUtil.checkZeroOrPositive(0, null));
        assertDoesNotThrow(() -> NumberUtil.checkZeroOrPositive(1, null));
        assertDoesNotThrow(() -> NumberUtil.checkZeroOrPositive(Integer.MAX_VALUE, null));

        assertDoesNotThrow(() -> NumberUtil.checkZeroOrPositive(0L, null));
        assertDoesNotThrow(() -> NumberUtil.checkZeroOrPositive(1L, null));
        assertDoesNotThrow(() -> NumberUtil.checkZeroOrPositive(Long.MAX_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkZeroOrPositive(-1, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkZeroOrPositive(-82, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkZeroOrPositive(Integer.MIN_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkZeroOrPositive(-1L, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkZeroOrPositive(-82L, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkZeroOrPositive(Long.MIN_VALUE, null));
    }

    @Test
    void testCheckPositive() {
        assertDoesNotThrow(() -> NumberUtil.checkPositive(1, null));
        assertDoesNotThrow(() -> NumberUtil.checkPositive(54, null));
        assertDoesNotThrow(() -> NumberUtil.checkPositive(Integer.MAX_VALUE, null));

        assertDoesNotThrow(() -> NumberUtil.checkPositive(1L, null));
        assertDoesNotThrow(() -> NumberUtil.checkPositive(9110L, null));
        assertDoesNotThrow(() -> NumberUtil.checkPositive(Long.MAX_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkPositive(0, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkPositive(-82, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkPositive(Integer.MIN_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkPositive(-1L, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkPositive(-82L, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkPositive(Long.MIN_VALUE, null));
    }

    @Test
    void testCheckRange() {
        assertDoesNotThrow(() -> NumberUtil.checkInRange(100, -1, 100, null));
        assertDoesNotThrow(() -> NumberUtil.checkInRange(52, 52, 70, null));
        assertDoesNotThrow(() -> NumberUtil.checkInRange(9110, Integer.MIN_VALUE, Integer.MAX_VALUE, null));

        assertDoesNotThrow(() -> NumberUtil.checkInRange(100L, -1L, 100L, null));
        assertDoesNotThrow(() -> NumberUtil.checkInRange(52L, 52L, 70L, null));
        assertDoesNotThrow(() -> NumberUtil.checkInRange(9110L, Long.MIN_VALUE, Long.MAX_VALUE, null));

        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkInRange(99, 100, 200, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkInRange(201, 100, 200, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkInRange(-1002, -1000, 900, null));

        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkInRange(99L, 100L, 200L, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkInRange(201L, 100L, 200L, null));
        assertThrows(IllegalArgumentException.class, () -> NumberUtil.checkInRange(-1002L, -1000L, 900L, null));
    }
}
