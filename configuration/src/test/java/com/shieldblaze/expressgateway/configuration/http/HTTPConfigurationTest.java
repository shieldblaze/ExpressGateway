/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.configuration.http;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class HTTPConfigurationTest {

    @Test
    void maxContentLengthTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxContentLength(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxContentLength(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxContentLength(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxContentLength(Long.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxContentLength(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxContentLength(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxContentLength(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxContentLength(Long.MAX_VALUE));
    }

    @Test
    void h2InitialWindowSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2InitialWindowSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2InitialWindowSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2InitialWindowSize(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setH2InitialWindowSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2InitialWindowSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2InitialWindowSize(Integer.MAX_VALUE));
    }

    @Test
    void h2MaxConcurrentStreamsTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxConcurrentStreams(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxConcurrentStreams(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxConcurrentStreams(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxConcurrentStreams(Long.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxConcurrentStreams(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxConcurrentStreams(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxConcurrentStreams(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxConcurrentStreams(Long.MAX_VALUE));
    }

    @Test
    void h2MaxHeaderListSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxHeaderListSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxHeaderListSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxHeaderListSize(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxHeaderListSize(Long.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxHeaderListSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxHeaderListSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxHeaderListSize(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxHeaderListSize(Long.MAX_VALUE));
    }

    @Test
    void h2MaxHeaderTableSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxHeaderTableSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxHeaderTableSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxHeaderTableSize(Integer.MIN_VALUE));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxHeaderTableSize(Long.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxHeaderTableSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxHeaderTableSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxHeaderTableSize(Integer.MAX_VALUE));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxHeaderTableSize(Long.MAX_VALUE));
    }

    @Test
    void h2MaxFrameSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxFrameSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxFrameSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setH2MaxFrameSize(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxFrameSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxFrameSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setH2MaxFrameSize(Integer.MAX_VALUE));
    }

    @Test
    void maxInitialLineLengthTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxInitialLineLength(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxInitialLineLength(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxInitialLineLength(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxInitialLineLength(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxInitialLineLength(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxInitialLineLength(Integer.MAX_VALUE));
    }

    @Test
    void maxHeaderSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxHeaderSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxHeaderSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxHeaderSize(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxHeaderSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxHeaderSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxHeaderSize(Integer.MAX_VALUE));
    }

    @Test
    void maxChunkSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxChunkSize(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxChunkSize(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setMaxChunkSize(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxChunkSize(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxChunkSize(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setMaxChunkSize(Integer.MAX_VALUE));
    }

    @Test
    void compressionThresholdTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setCompressionThreshold(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setCompressionThreshold(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setCompressionThreshold(0));
        assertDoesNotThrow(() -> new HTTPConfiguration().setCompressionThreshold(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setCompressionThreshold(1024));
        assertDoesNotThrow(() -> new HTTPConfiguration().setCompressionThreshold(Integer.MAX_VALUE));
    }

    @Test
    void deflateCompressionLevelTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setDeflateCompressionLevel(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setDeflateCompressionLevel(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setDeflateCompressionLevel(0));
        assertDoesNotThrow(() -> new HTTPConfiguration().setDeflateCompressionLevel(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setDeflateCompressionLevel(9));
    }

    @Test
    void brotliCompressionLevelTest() {
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setBrotliCompressionLevel(-1));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setBrotliCompressionLevel(0));
        assertThrows(IllegalArgumentException.class, () -> new HTTPConfiguration().setBrotliCompressionLevel(Integer.MIN_VALUE));


        assertDoesNotThrow(() -> new HTTPConfiguration().setBrotliCompressionLevel(1));
        assertDoesNotThrow(() -> new HTTPConfiguration().setBrotliCompressionLevel(8));
    }
}
