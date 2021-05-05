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

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;

import java.io.IOException;

/**
 * Configuration for HTTP
 */
public final class HTTPConfiguration {

    @JsonProperty("maxContentLength")
    private long maxContentLength;

    @JsonProperty("h2InitialWindowSize")
    private int h2InitialWindowSize;

    @JsonProperty("h2MaxConcurrentStreams")
    private long h2MaxConcurrentStreams;

    @JsonProperty("h2MaxHeaderListSize")
    private long h2MaxHeaderListSize;

    @JsonProperty("h2MaxHeaderTableSize")
    private long h2MaxHeaderTableSize;

    @JsonProperty("h2MaxFrameSize")
    private int h2MaxFrameSize;

    @JsonProperty("maxInitialLineLength")
    private int maxInitialLineLength;

    @JsonProperty("maxHeaderSize")
    private int maxHeaderSize;

    @JsonProperty("maxChunkSize")
    private int maxChunkSize;

    @JsonProperty("compressionThreshold")
    private int compressionThreshold;

    @JsonProperty("deflateCompressionLevel")
    private int deflateCompressionLevel;

    @JsonProperty("brotliCompressionLevel")
    private int brotliCompressionLevel;

    public static final HTTPConfiguration DEFAULT = new HTTPConfiguration();

    static {
        DEFAULT.maxContentLength = 500000000;
        DEFAULT.h2InitialWindowSize = 65535;
        DEFAULT.h2MaxConcurrentStreams = 1000;
        DEFAULT.h2MaxHeaderListSize = 262144;
        DEFAULT.h2MaxHeaderTableSize = 65536;
        DEFAULT.h2MaxFrameSize = 16777215;
        DEFAULT.maxInitialLineLength = 1024 * 8;
        DEFAULT.maxHeaderSize = 1024 * 8;
        DEFAULT.maxChunkSize = 1024 * 8;
        DEFAULT.compressionThreshold = 1024;
        DEFAULT.deflateCompressionLevel = 6;
        DEFAULT.brotliCompressionLevel = 4;
    }

    HTTPConfiguration() {
        // Prevent outside initialization
    }

    public long maxContentLength() {
        return maxContentLength;
    }

    HTTPConfiguration setMaxContentLength(long maxContentLength) {
        this.maxContentLength = Number.checkPositive(maxContentLength, "maxContentLength");
        return this;
    }

    public int h2InitialWindowSize() {
        return h2InitialWindowSize;
    }

    HTTPConfiguration setH2InitialWindowSize(int h2InitialWindowSize) {
        this.h2InitialWindowSize = Number.checkPositive(h2InitialWindowSize, "h2InitialWindowSize");
        return this;
    }

    public long h2MaxConcurrentStreams() {
        return h2MaxConcurrentStreams;
    }

    HTTPConfiguration setH2MaxConcurrentStreams(long h2MaxConcurrentStreams) {
        this.h2MaxConcurrentStreams = Number.checkPositive(h2MaxConcurrentStreams, "h2MaxConcurrentStreams");
        return this;
    }

    public long h2MaxHeaderListSize() {
        return h2MaxHeaderListSize;
    }

    HTTPConfiguration setH2MaxHeaderListSize(long h2MaxHeaderSizeList) {
        this.h2MaxHeaderListSize = Number.checkPositive(h2MaxHeaderSizeList, "h2MaxHeaderListSize");
        return this;
    }

    public long h2MaxHeaderTableSize() {
        return h2MaxHeaderTableSize;
    }

    HTTPConfiguration setH2MaxHeaderTableSize(long h2MaxHeaderTableSize) {
        this.h2MaxHeaderTableSize = Number.checkPositive(h2MaxHeaderTableSize, "h2MaxHeaderTableSize");
        return this;
    }

    public int h2MaxFrameSize() {
        return h2MaxFrameSize;
    }

    HTTPConfiguration setH2MaxFrameSize(int h2MaxFrameSize) {
        this.h2MaxFrameSize = Number.checkPositive(h2MaxFrameSize, "h2MaxFrameSize");
        return this;
    }

    public int maxInitialLineLength() {
        return maxInitialLineLength;
    }

    HTTPConfiguration setMaxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = Number.checkPositive(maxInitialLineLength, "maxInitialLineLength");
        return this;
    }

    public int maxHeaderSize() {
        return maxHeaderSize;
    }

    HTTPConfiguration setMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = Number.checkPositive(maxHeaderSize, "maxHeaderSize");
        return this;
    }

    public int maxChunkSize() {
        return maxChunkSize;
    }

    HTTPConfiguration setMaxChunkSize(int maxChunkSize) {
        this.maxChunkSize = Number.checkPositive(maxChunkSize, "maxChunkSize");
        return this;
    }

    public int compressionThreshold() {
        return compressionThreshold;
    }

    HTTPConfiguration setCompressionThreshold(int compressionThreshold) {
        this.compressionThreshold = Number.checkZeroOrPositive(compressionThreshold, "compressionThreshold");
        return this;
    }

    public int deflateCompressionLevel() {
        return deflateCompressionLevel;
    }

    HTTPConfiguration setDeflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = Number.checkRange(deflateCompressionLevel, 0, 9, "deflateCompressionLevel");
        return this;
    }

    public int brotliCompressionLevel() {
        return brotliCompressionLevel;
    }

    HTTPConfiguration setBrotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = Number.checkRange(brotliCompressionLevel, 1, 11, "brotliCompressionLevel");
        return this;
    }

    /**
     * Save this configuration to the file
     *
     * @throws IOException If an error occurs during saving
     */
    public void save() throws IOException {
        ConfigurationMarshaller.save("HTTPConfiguration.json", this);
    }

    /**
     * Load this configuration from the file
     *
     * @return {@link HTTPConfiguration} Instance
     * @throws IOException If an error occurs during loading
     */
    public static HTTPConfiguration load() throws IOException {
        return ConfigurationMarshaller.load("HTTPConfiguration.json", HTTPConfiguration.class);
    }
}
