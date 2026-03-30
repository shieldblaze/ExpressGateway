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
package com.shieldblaze.expressgateway.testing;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;

import java.nio.charset.StandardCharsets;
import java.util.Objects;

/**
 * Fluent assertion helper for Netty {@link ByteBuf} instances.
 * Verifies content, capacity, and reference count without modifying reader/writer index.
 *
 * <p>Example:</p>
 * <pre>
 *   ByteBufAssert.assertThat(buf)
 *       .isReadable()
 *       .hasReadableBytes(11)
 *       .containsUtf8("hello world");
 * </pre>
 */
public final class ByteBufAssert {

    private final ByteBuf buf;

    private ByteBufAssert(ByteBuf buf) {
        this.buf = Objects.requireNonNull(buf, "ByteBuf must not be null");
    }

    /**
     * Start a fluent assertion chain for the given ByteBuf.
     */
    public static ByteBufAssert assertThat(ByteBuf buf) {
        return new ByteBufAssert(buf);
    }

    /**
     * Assert the buffer has readable bytes.
     */
    public ByteBufAssert isReadable() {
        if (!buf.isReadable()) {
            throw new AssertionError("Expected ByteBuf to be readable but readableBytes=0");
        }
        return this;
    }

    /**
     * Assert the buffer is not readable (empty).
     */
    public ByteBufAssert isNotReadable() {
        if (buf.isReadable()) {
            throw new AssertionError("Expected ByteBuf to be not readable but readableBytes=" + buf.readableBytes());
        }
        return this;
    }

    /**
     * Assert exact number of readable bytes.
     */
    public ByteBufAssert hasReadableBytes(int expected) {
        int actual = buf.readableBytes();
        if (actual != expected) {
            throw new AssertionError("Expected " + expected + " readable bytes but found " + actual);
        }
        return this;
    }

    /**
     * Assert readable bytes are at least the given minimum.
     */
    public ByteBufAssert hasMinReadableBytes(int min) {
        int actual = buf.readableBytes();
        if (actual < min) {
            throw new AssertionError("Expected at least " + min + " readable bytes but found " + actual);
        }
        return this;
    }

    /**
     * Assert the buffer contains the given UTF-8 string starting from the current reader index.
     * Does not advance the reader index.
     */
    public ByteBufAssert containsUtf8(String expected) {
        String actual = buf.toString(buf.readerIndex(), buf.readableBytes(), StandardCharsets.UTF_8);
        if (!actual.contains(expected)) {
            throw new AssertionError("Expected ByteBuf to contain '" + expected +
                    "' but content was: '" + truncate(actual, 200) + "'");
        }
        return this;
    }

    /**
     * Assert the buffer content equals the given UTF-8 string exactly.
     * Does not advance the reader index.
     */
    public ByteBufAssert equalsUtf8(String expected) {
        String actual = buf.toString(buf.readerIndex(), buf.readableBytes(), StandardCharsets.UTF_8);
        if (!actual.equals(expected)) {
            throw new AssertionError("Expected ByteBuf to equal '" + truncate(expected, 100) +
                    "' but was: '" + truncate(actual, 100) + "'");
        }
        return this;
    }

    /**
     * Assert the buffer content equals the given byte array.
     * Does not advance the reader index.
     */
    public ByteBufAssert equalsBytes(byte[] expected) {
        byte[] actual = ByteBufUtil.getBytes(buf, buf.readerIndex(), buf.readableBytes(), false);
        if (actual.length != expected.length) {
            throw new AssertionError("Expected " + expected.length + " bytes but found " + actual.length);
        }
        for (int i = 0; i < expected.length; i++) {
            if (actual[i] != expected[i]) {
                throw new AssertionError("Byte mismatch at index " + i +
                        ": expected 0x" + Integer.toHexString(expected[i] & 0xFF) +
                        " but found 0x" + Integer.toHexString(actual[i] & 0xFF));
            }
        }
        return this;
    }

    /**
     * Assert the reference count of the buffer.
     */
    public ByteBufAssert hasRefCnt(int expected) {
        int actual = buf.refCnt();
        if (actual != expected) {
            throw new AssertionError("Expected refCnt=" + expected + " but found " + actual);
        }
        return this;
    }

    /**
     * Assert the buffer has been released (refCnt == 0).
     */
    public ByteBufAssert isReleased() {
        return hasRefCnt(0);
    }

    /**
     * Assert the buffer capacity is at least the given value.
     */
    public ByteBufAssert hasMinCapacity(int minCapacity) {
        int actual = buf.capacity();
        if (actual < minCapacity) {
            throw new AssertionError("Expected capacity >= " + minCapacity + " but found " + actual);
        }
        return this;
    }

    /**
     * Return the underlying ByteBuf for further custom assertions.
     */
    public ByteBuf byteBuf() {
        return buf;
    }

    private static String truncate(String s, int maxLen) {
        if (s.length() <= maxLen) {
            return s;
        }
        return s.substring(0, maxLen) + "...<truncated>";
    }
}
