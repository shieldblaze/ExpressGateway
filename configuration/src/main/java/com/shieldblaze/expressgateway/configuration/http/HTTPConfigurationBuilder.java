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

package com.shieldblaze.expressgateway.configuration.http;

/**
 * Builder for {@link HTTPConfiguration}
 */
public final class HTTPConfigurationBuilder {
    private long maxContentLength;
    private int h2InitialWindowSize;
    private long h2MaxConcurrentStreams;
    private long h2MaxHeaderListSize;
    private long h2MaxHeaderTableSize;
    private int h2MaxFrameSize;
    private int maxInitialLineLength;
    private int maxHeaderSize;
    private int maxChunkSize;
    private int compressionThreshold;
    private int deflateCompressionLevel;
    private int brotliCompressionLevel;

    private HTTPConfigurationBuilder() {
        // Prevent outside initialization
    }

    public static HTTPConfigurationBuilder newBuilder() {
        return new HTTPConfigurationBuilder();
    }

    public HTTPConfigurationBuilder withMaxContentLength(long maxContentLength) {
        this.maxContentLength = maxContentLength;
        return this;
    }

    public HTTPConfigurationBuilder withH2InitialWindowSize(int h2InitialWindowSize) {
        this.h2InitialWindowSize = h2InitialWindowSize;
        return this;
    }

    public HTTPConfigurationBuilder withH2MaxConcurrentStreams(long h2MaxConcurrentStreams) {
        this.h2MaxConcurrentStreams = h2MaxConcurrentStreams;
        return this;
    }

    public HTTPConfigurationBuilder withH2MaxHeaderListSize(long h2MaxHeaderListSize) {
        this.h2MaxHeaderListSize = h2MaxHeaderListSize;
        return this;
    }

    public HTTPConfigurationBuilder withH2MaxHeaderTableSize(long h2MaxHeaderTableSize) {
        this.h2MaxHeaderTableSize = h2MaxHeaderTableSize;
        return this;
    }

    public HTTPConfigurationBuilder withH2MaxFrameSize(int h2MaxFrameSize) {
        this.h2MaxFrameSize = h2MaxFrameSize;
        return this;
    }

    public HTTPConfigurationBuilder withMaxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = maxInitialLineLength;
        return this;
    }

    public HTTPConfigurationBuilder withMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = maxHeaderSize;
        return this;
    }

    public HTTPConfigurationBuilder withMaxChunkSize(int maxChunkSize) {
        this.maxChunkSize = maxChunkSize;
        return this;
    }

    public HTTPConfigurationBuilder withCompressionThreshold(int compressionThreshold) {
        this.compressionThreshold = compressionThreshold;
        return this;
    }

    public HTTPConfigurationBuilder withDeflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = deflateCompressionLevel;
        return this;
    }

    public HTTPConfigurationBuilder withBrotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = brotliCompressionLevel;
        return this;
    }

    public HTTPConfiguration build() {
        return new HTTPConfiguration()
                .setMaxContentLength(maxContentLength)
                .setH2InitialWindowSize(h2InitialWindowSize)
                .setH2MaxConcurrentStreams(h2MaxConcurrentStreams)
                .setH2MaxHeaderListSize(h2MaxHeaderListSize)
                .setH2MaxHeaderTableSize(h2MaxHeaderTableSize)
                .setH2MaxFrameSize(h2MaxFrameSize)
                .setMaxInitialLineLength(maxInitialLineLength)
                .setMaxHeaderSize(maxHeaderSize)
                .setMaxChunkSize(maxChunkSize)
                .setCompressionThreshold(compressionThreshold)
                .setDeflateCompressionLevel(deflateCompressionLevel)
                .setBrotliCompressionLevel(brotliCompressionLevel);
    }
}
