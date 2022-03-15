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

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;

/**
 * Configuration for HTTP
 */
public final class HttpConfiguration implements Configuration {

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

    public static final HttpConfiguration DEFAULT = new HttpConfiguration();

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

    HttpConfiguration() {
        // Prevent outside initialization
    }

    public long maxContentLength() {
        return maxContentLength;
    }

    HttpConfiguration setMaxContentLength(long maxContentLength) {
        this.maxContentLength = maxContentLength;
        return this;
    }

    public int h2InitialWindowSize() {
        return h2InitialWindowSize;
    }

    HttpConfiguration setH2InitialWindowSize(int h2InitialWindowSize) {
        this.h2InitialWindowSize = h2InitialWindowSize;
        return this;
    }

    public long h2MaxConcurrentStreams() {
        return h2MaxConcurrentStreams;
    }

    HttpConfiguration setH2MaxConcurrentStreams(long h2MaxConcurrentStreams) {
        this.h2MaxConcurrentStreams = h2MaxConcurrentStreams;
        return this;
    }

    public long h2MaxHeaderListSize() {
        return h2MaxHeaderListSize;
    }

    HttpConfiguration setH2MaxHeaderListSize(long h2MaxHeaderSizeList) {
        this.h2MaxHeaderListSize = h2MaxHeaderSizeList;
        return this;
    }

    public long h2MaxHeaderTableSize() {
        return h2MaxHeaderTableSize;
    }

    HttpConfiguration setH2MaxHeaderTableSize(long h2MaxHeaderTableSize) {
        this.h2MaxHeaderTableSize = h2MaxHeaderTableSize;
        return this;
    }

    public int h2MaxFrameSize() {
        return h2MaxFrameSize;
    }

    HttpConfiguration setH2MaxFrameSize(int h2MaxFrameSize) {
        this.h2MaxFrameSize = h2MaxFrameSize;
        return this;
    }

    public int maxInitialLineLength() {
        return maxInitialLineLength;
    }

    HttpConfiguration setMaxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = maxInitialLineLength;
        return this;
    }

    public int maxHeaderSize() {
        return maxHeaderSize;
    }

    HttpConfiguration setMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = maxHeaderSize;
        return this;
    }

    public int maxChunkSize() {
        return maxChunkSize;
    }

    HttpConfiguration setMaxChunkSize(int maxChunkSize) {
        this.maxChunkSize = maxChunkSize;
        return this;
    }

    public int compressionThreshold() {
        return compressionThreshold;
    }

    HttpConfiguration setCompressionThreshold(int compressionThreshold) {
        this.compressionThreshold = compressionThreshold;
        return this;
    }

    public int deflateCompressionLevel() {
        return deflateCompressionLevel;
    }

    HttpConfiguration setDeflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = deflateCompressionLevel;
        return this;
    }

    public int brotliCompressionLevel() {
        return brotliCompressionLevel;
    }

    HttpConfiguration setBrotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = brotliCompressionLevel;
        return this;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    public HttpConfiguration validate() throws IllegalArgumentException {
        NumberUtil.checkPositive(maxContentLength, "maxContentLength");
        NumberUtil.checkPositive(h2InitialWindowSize, "h2InitialWindowSize");
        NumberUtil.checkPositive(h2MaxConcurrentStreams, "h2MaxConcurrentStreams");
        NumberUtil.checkPositive(h2MaxHeaderListSize, "h2MaxHeaderListSize");
        NumberUtil.checkPositive(h2MaxHeaderTableSize, "h2MaxHeaderTableSize");
        NumberUtil.checkPositive(h2MaxFrameSize, "h2MaxFrameSize");
        NumberUtil.checkPositive(maxInitialLineLength, "maxInitialLineLength");
        NumberUtil.checkPositive(maxHeaderSize, "maxHeaderSize");
        NumberUtil.checkPositive(maxChunkSize, "maxChunkSize");
        NumberUtil.checkZeroOrPositive(compressionThreshold, "compressionThreshold");
        NumberUtil.checkInRange(deflateCompressionLevel, 0, 9, "deflateCompressionLevel");
        NumberUtil.checkInRange(brotliCompressionLevel, 1, 11, "brotliCompressionLevel");
        return this;
    }

    @Override
    public String name() {
        return "HttpConfiguration";
    }
}
