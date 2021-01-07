/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.configuration;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.common.utils.Number;

/**
 * Configuration for HTTP
 */
public final class HTTPConfiguration implements Configuration {

    public static final HTTPConfiguration EMPTY_INSTANCE = new HTTPConfiguration();

    @Expose
    @JsonProperty("maxContentLength")
    private long maxContentLength;

    @Expose
    @JsonProperty("h2InitialWindowSize")
    private int h2InitialWindowSize;

    @Expose
    @JsonProperty("h2MaxConcurrentStreams")
    private long h2MaxConcurrentStreams;

    @Expose
    @JsonProperty("h2MaxHeaderSizeList")
    private long h2MaxHeaderSizeList;

    @Expose
    @JsonProperty("h2MaxHeaderTableSize")
    private long h2MaxHeaderTableSize;

    @Expose
    @JsonProperty("h2MaxFrameSize")
    private int h2MaxFrameSize;

    @Expose
    @JsonProperty("maxInitialLineLength")
    private int maxInitialLineLength;

    @Expose
    @JsonProperty("maxHeaderSize")
    private int maxHeaderSize;

    @Expose
    @JsonProperty("maxChunkSize")
    private int maxChunkSize;

    @Expose
    @JsonProperty("compressionThreshold")
    private int compressionThreshold;

    @Expose
    @JsonProperty("deflateCompressionLevel")
    private int deflateCompressionLevel;

    @Expose
    @JsonProperty("brotliCompressionLevel")
    private int brotliCompressionLevel;

    public long maxContentLength() {
        return maxContentLength;
    }

    public HTTPConfiguration maxContentLength(long maxContentLength) {
        this.maxContentLength = Number.checkPositive(maxContentLength, "maxContentLength");
        return this;
    }

    public int h2InitialWindowSize() {
        return h2InitialWindowSize;
    }

    public HTTPConfiguration h2InitialWindowSize(int h2InitialWindowSize) {
        this.h2InitialWindowSize = Number.checkPositive(h2InitialWindowSize, "h2InitialWindowSize");
        return this;
    }

    public long h2MaxConcurrentStreams() {
        return h2MaxConcurrentStreams;
    }

    public HTTPConfiguration h2MaxConcurrentStreams(long h2MaxConcurrentStreams) {
        this.h2MaxConcurrentStreams = Number.checkPositive(h2MaxConcurrentStreams, "h2MaxConcurrentStreams");
        return this;
    }

    public long h2MaxHeaderSizeList() {
        return h2MaxHeaderSizeList;
    }

    public HTTPConfiguration h2MaxHeaderSizeList(long h2MaxHeaderSizeList) {
        this.h2MaxHeaderSizeList = Number.checkPositive(h2MaxHeaderSizeList, "h2MaxHeaderSizeList");
        return this;
    }

    public long h2MaxHeaderTableSize() {
        return h2MaxHeaderTableSize;
    }

    public HTTPConfiguration h2MaxHeaderTableSize(long h2MaxHeaderTableSize) {
        this.h2MaxHeaderTableSize = Number.checkPositive(h2MaxHeaderTableSize, "h2MaxHeaderTableSize");
        return this;
    }

    public int h2MaxFrameSize() {
        return h2MaxFrameSize;
    }

    public HTTPConfiguration h2MaxFrameSize(int h2MaxFrameSize) {
        this.h2MaxFrameSize = Number.checkPositive(h2MaxFrameSize, "h2MaxFrameSize");
        return this;
    }

    public int maxInitialLineLength() {
        return maxInitialLineLength;
    }

    public HTTPConfiguration maxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = Number.checkPositive(maxInitialLineLength, "maxInitialLineLength");
        return this;
    }

    public int maxHeaderSize() {
        return maxHeaderSize;
    }

    public HTTPConfiguration maxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = Number.checkPositive(maxHeaderSize, "maxHeaderSize");
        return this;
    }

    public int maxChunkSize() {
        return maxChunkSize;
    }

    public HTTPConfiguration maxChunkSize(int maxChunkSize) {
        this.maxChunkSize = Number.checkPositive(maxChunkSize, "maxChunkSize");
        return this;
    }

    public int compressionThreshold() {
        return compressionThreshold;
    }

    public HTTPConfiguration compressionThreshold(int compressionThreshold) {
        this.compressionThreshold = Number.checkZeroOrPositive(compressionThreshold, "compressionThreshold");
        return this;
    }

    public int deflateCompressionLevel() {
        return deflateCompressionLevel;
    }

    public HTTPConfiguration deflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = Number.checkRange(deflateCompressionLevel, 0, 9, "deflateCompressionLevel");
        return this;
    }

    public int brotliCompressionLevel() {
        return brotliCompressionLevel;
    }

    public HTTPConfiguration brotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = Number.checkRange(brotliCompressionLevel, 1, 11, "brotliCompressionLevel");
        return this;
    }

    @Override
    public String name() {
        return "HTTP";
    }

    @Override
    public void validate() throws IllegalArgumentException {
        Number.checkPositive(maxContentLength, "maxContentLength");
        Number.checkPositive(h2InitialWindowSize, "h2InitialWindowSize");
        Number.checkPositive(h2MaxConcurrentStreams, "h2MaxConcurrentStreams");
        Number.checkPositive(h2MaxHeaderSizeList, "h2MaxHeaderSizeList");
        Number.checkPositive(h2MaxHeaderTableSize, "h2MaxHeaderTableSize");
        Number.checkPositive(h2MaxFrameSize, "h2MaxFrameSize");
        Number.checkPositive(maxInitialLineLength, "maxInitialLineLength");
        Number.checkPositive(maxHeaderSize, "maxHeaderSize");
        Number.checkPositive(maxChunkSize, "maxChunkSize");
        Number.checkZeroOrPositive(compressionThreshold, "compressionThreshold");
        Number.checkRange(deflateCompressionLevel, 0, 9, "deflateCompressionLevel");
        Number.checkRange(brotliCompressionLevel, 1, 11, "brotliCompressionLevel");
    }
}
