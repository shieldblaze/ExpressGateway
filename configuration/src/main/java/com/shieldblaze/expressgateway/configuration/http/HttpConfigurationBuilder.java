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
 * Builder for {@link HttpConfiguration}
 */
public final class HttpConfigurationBuilder {
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

    private HttpConfigurationBuilder() {
        // Prevent outside initialization
    }

    public static HttpConfigurationBuilder newBuilder() {
        return new HttpConfigurationBuilder();
    }

    public HttpConfigurationBuilder withMaxContentLength(long maxContentLength) {
        this.maxContentLength = maxContentLength;
        return this;
    }

    public HttpConfigurationBuilder withH2InitialWindowSize(int h2InitialWindowSize) {
        this.h2InitialWindowSize = h2InitialWindowSize;
        return this;
    }

    public HttpConfigurationBuilder withH2MaxConcurrentStreams(long h2MaxConcurrentStreams) {
        this.h2MaxConcurrentStreams = h2MaxConcurrentStreams;
        return this;
    }

    public HttpConfigurationBuilder withH2MaxHeaderListSize(long h2MaxHeaderListSize) {
        this.h2MaxHeaderListSize = h2MaxHeaderListSize;
        return this;
    }

    public HttpConfigurationBuilder withH2MaxHeaderTableSize(long h2MaxHeaderTableSize) {
        this.h2MaxHeaderTableSize = h2MaxHeaderTableSize;
        return this;
    }

    public HttpConfigurationBuilder withH2MaxFrameSize(int h2MaxFrameSize) {
        this.h2MaxFrameSize = h2MaxFrameSize;
        return this;
    }

    public HttpConfigurationBuilder withMaxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = maxInitialLineLength;
        return this;
    }

    public HttpConfigurationBuilder withMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = maxHeaderSize;
        return this;
    }

    public HttpConfigurationBuilder withMaxChunkSize(int maxChunkSize) {
        this.maxChunkSize = maxChunkSize;
        return this;
    }

    public HttpConfigurationBuilder withCompressionThreshold(int compressionThreshold) {
        this.compressionThreshold = compressionThreshold;
        return this;
    }

    public HttpConfigurationBuilder withDeflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = deflateCompressionLevel;
        return this;
    }

    public HttpConfigurationBuilder withBrotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = brotliCompressionLevel;
        return this;
    }

    public HttpConfiguration build() {
        return new HttpConfiguration()
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
                .setBrotliCompressionLevel(brotliCompressionLevel)
                .validate(); // Validate the configuration
    }
}
