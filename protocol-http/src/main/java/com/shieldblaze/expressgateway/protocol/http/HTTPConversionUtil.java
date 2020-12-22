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
/*
 * Copyright 2014 The Netty Project
 *
 * The Netty Project licenses this file to you under the Apache License, version 2.0 (the
 * "License"); you may not use this file except in compliance with the License. You may obtain a
 * copy of the License at:
 *
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software distributed under the License
 * is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express
 * or implied. See the License for the specific language governing permissions and limitations under
 * the License.
 */
package com.shieldblaze.expressgateway.protocol.http;

import io.netty.buffer.ByteBuf;
import io.netty.handler.codec.UnsupportedValueConverter;
import io.netty.handler.codec.http.CustomFullHttpRequest;
import io.netty.handler.codec.http.CustomFullHttpResponse;
import io.netty.handler.codec.http.CustomHttpRequest;
import io.netty.handler.codec.http.CustomHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpFrame;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpMessage;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpUtil;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.CharSequenceMap;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.EmptyHttp2Headers;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.util.AsciiString;
import io.netty.util.internal.InternalThreadLocalMap;

import java.net.URI;
import java.util.Iterator;
import java.util.List;
import java.util.Map;

import static io.netty.handler.codec.http.HttpHeaderNames.CONNECTION;
import static io.netty.handler.codec.http.HttpHeaderNames.COOKIE;
import static io.netty.handler.codec.http.HttpHeaderNames.TE;
import static io.netty.handler.codec.http.HttpHeaderValues.TRAILERS;
import static io.netty.handler.codec.http.HttpResponseStatus.parseLine;
import static io.netty.handler.codec.http.HttpUtil.isAsteriskForm;
import static io.netty.handler.codec.http.HttpUtil.isOriginForm;
import static io.netty.handler.codec.http2.Http2Error.PROTOCOL_ERROR;
import static io.netty.handler.codec.http2.Http2Exception.connectionError;
import static io.netty.util.AsciiString.EMPTY_STRING;
import static io.netty.util.AsciiString.contentEqualsIgnoreCase;
import static io.netty.util.AsciiString.indexOf;
import static io.netty.util.AsciiString.trim;
import static io.netty.util.ByteProcessor.FIND_COMMA;
import static io.netty.util.ByteProcessor.FIND_SEMI_COLON;
import static io.netty.util.internal.StringUtil.isNullOrEmpty;
import static io.netty.util.internal.StringUtil.length;
import static io.netty.util.internal.StringUtil.unescapeCsvFields;

public class HTTPConversionUtil {

    private static final AsciiString EMPTY_REQUEST_PATH = AsciiString.cached("/");
    /**
     * The set of headers that should not be directly copied when converting headers from HTTP to HTTP/2.
     */
    private static final CharSequenceMap<AsciiString> HTTP_TO_HTTP2_HEADER_BLACKLIST = new CharSequenceMap<>();

    static {
        HTTP_TO_HTTP2_HEADER_BLACKLIST.add(CONNECTION, EMPTY_STRING);
        @SuppressWarnings("deprecation")
        AsciiString keepAlive = HttpHeaderNames.KEEP_ALIVE;
        HTTP_TO_HTTP2_HEADER_BLACKLIST.add(keepAlive, EMPTY_STRING);
        @SuppressWarnings("deprecation")
        AsciiString proxyConnection = HttpHeaderNames.PROXY_CONNECTION;
        HTTP_TO_HTTP2_HEADER_BLACKLIST.add(proxyConnection, EMPTY_STRING);
        HTTP_TO_HTTP2_HEADER_BLACKLIST.add(HttpHeaderNames.TRANSFER_ENCODING, EMPTY_STRING);
        HTTP_TO_HTTP2_HEADER_BLACKLIST.add(HttpHeaderNames.HOST, EMPTY_STRING);
        HTTP_TO_HTTP2_HEADER_BLACKLIST.add(HttpHeaderNames.UPGRADE, EMPTY_STRING);
        HTTP_TO_HTTP2_HEADER_BLACKLIST.add(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), EMPTY_STRING);
        HTTP_TO_HTTP2_HEADER_BLACKLIST.add(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), EMPTY_STRING);
        HTTP_TO_HTTP2_HEADER_BLACKLIST.add(HttpConversionUtil.ExtensionHeaderNames.PATH.text(), EMPTY_STRING);
    }

    public static FullHttpResponse toFullHttpResponse(long id, Http2Headers http2Headers, ByteBuf content, HttpVersion httpVersion) throws Http2Exception {
        HttpResponseStatus status = parseStatus(http2Headers.status());
        FullHttpResponse msg = new CustomFullHttpResponse(httpVersion, status, content, HttpFrame.Protocol.H2, id);
        try {
            addHttp2ToHttpHeaders(http2Headers, msg.headers(), false, false);
        } catch (Exception e) {
            msg.release();
            throw e;
        }
        return msg;
    }

    public static FullHttpRequest toFullHttpRequest(long id, Http2Headers http2Headers, ByteBuf content) {
        CharSequence method = http2Headers.method();
        CharSequence path = http2Headers.path();

        FullHttpRequest msg = new CustomFullHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.valueOf(method.toString()), path.toString(),
                content, HttpFrame.Protocol.H2, id);
        try {
            addHttp2ToHttpHeaders(http2Headers, msg.headers(), false, true);
        } catch (Exception ex) {
            msg.release();
            throw ex;
        }
        return msg;
    }

    public static HttpRequest toHttpRequest(long id, Http2Headers http2Headers) {
        CharSequence method = http2Headers.method();
        CharSequence path = http2Headers.path();

        HttpRequest msg = new CustomHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.valueOf(method.toString()), path.toString(), HttpFrame.Protocol.H2, id);
        addHttp2ToHttpHeaders(http2Headers, msg.headers(), false, true);
        return msg;
    }

    public static HttpResponse toHttpResponse(long id, Http2Headers http2Headers, HttpVersion httpVersion) throws Http2Exception {
        HttpResponseStatus status = parseStatus(http2Headers.status());
        HttpResponse msg = new CustomHttpResponse(httpVersion, status, HttpFrame.Protocol.H2, id);
        addHttp2ToHttpHeaders(http2Headers, msg.headers(), false, false);
        return msg;
    }

    public static void addHttp2ToHttpHeaders(Http2Headers inputHeaders, HttpHeaders outputHeaders, boolean isTrailer, boolean isRequest) {
        HTTP2ToHTTPTranslation.translateHeaders(inputHeaders, outputHeaders, isRequest);

        outputHeaders.remove(HttpHeaderNames.TRANSFER_ENCODING);
        outputHeaders.remove(HttpHeaderNames.TRAILER);
        if (!isTrailer) {
            HttpUtil.setKeepAlive(outputHeaders, HttpVersion.HTTP_1_1, true);
        }
    }

    public static Http2Headers toHttp2Headers(HttpMessage in) {
        HttpHeaders inHeaders = in.headers();
        Http2Headers out = new DefaultHttp2Headers(false, inHeaders.size());
        if (in instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) in;
            URI requestTargetUri = URI.create(request.uri());
            out.path(toHttp2Path(requestTargetUri));
            out.method(request.method().asciiName());

            if (!isOriginForm(requestTargetUri) && !isAsteriskForm(requestTargetUri)) {
                // Attempt to take from HOST header before taking from the request-line
                String host = inHeaders.getAsString(HttpHeaderNames.HOST);
                setHttp2Authority((host == null || host.isEmpty()) ? requestTargetUri.getAuthority() : host, out);
            }
        } else if (in instanceof HttpResponse) {
            HttpResponse response = (HttpResponse) in;
            out.status(response.status().codeAsText());
        }

        // Add the HTTP headers which have not been consumed above
        toHttp2Headers(inHeaders, out);
        return out;
    }

    public static Http2Headers toHttp2Headers(HttpHeaders inHeaders) {
        if (inHeaders.isEmpty()) {
            return EmptyHttp2Headers.INSTANCE;
        }

        Http2Headers out = new DefaultHttp2Headers(false, inHeaders.size());
        toHttp2Headers(inHeaders, out);
        return out;
    }

    private static void toHttp2HeadersFilterTE(Map.Entry<CharSequence, CharSequence> entry, Http2Headers out) {
        if (indexOf(entry.getValue(), ',', 0) == -1) {
            if (contentEqualsIgnoreCase(trim(entry.getValue()), TRAILERS)) {
                out.add(TE, TRAILERS);
            }
        } else {
            List<CharSequence> teValues = unescapeCsvFields(entry.getValue());
            for (CharSequence teValue : teValues) {
                if (contentEqualsIgnoreCase(trim(teValue), TRAILERS)) {
                    out.add(TE, TRAILERS);
                    break;
                }
            }
        }
    }

    public static void toHttp2Headers(HttpHeaders inHeaders, Http2Headers out) {
        Iterator<Map.Entry<CharSequence, CharSequence>> iter = inHeaders.iteratorCharSequence();
        // Choose 8 as a default size because it is unlikely we will see more than 4 Connection headers values, but
        // still allowing for "enough" space in the map to reduce the chance of hash code collision.
        CharSequenceMap<AsciiString> connectionBlacklist = toLowercaseMap(inHeaders.valueCharSequenceIterator(CONNECTION));
        while (iter.hasNext()) {
            Map.Entry<CharSequence, CharSequence> entry = iter.next();
            final AsciiString aName = AsciiString.of(entry.getKey()).toLowerCase();
            if (!HTTP_TO_HTTP2_HEADER_BLACKLIST.contains(aName) && !connectionBlacklist.contains(aName)) {
                // https://tools.ietf.org/html/rfc7540#section-8.1.2.2 makes a special exception for TE
                if (aName.contentEqualsIgnoreCase(TE)) {
                    toHttp2HeadersFilterTE(entry, out);
                } else if (aName.contentEqualsIgnoreCase(COOKIE)) {
                    AsciiString value = AsciiString.of(entry.getValue());
                    // split up cookies to allow for better compression
                    // https://tools.ietf.org/html/rfc7540#section-8.1.2.5
                    try {
                        int index = value.forEachByte(FIND_SEMI_COLON);
                        if (index != -1) {
                            int start = 0;
                            do {
                                out.add(COOKIE, value.subSequence(start, index, false));
                                // skip 2 characters "; " (see https://tools.ietf.org/html/rfc6265#section-4.2.1)
                                start = index + 2;
                            } while (start < value.length() && (index = value.forEachByte(start, value.length() - start, FIND_SEMI_COLON)) != -1);
                            if (start >= value.length()) {
                                throw new IllegalArgumentException("cookie value is of unexpected format: " + value);
                            }
                            out.add(COOKIE, value.subSequence(start, value.length(), false));
                        } else {
                            out.add(COOKIE, value);
                        }
                    } catch (Exception e) {
                        // This is not expect to happen because FIND_SEMI_COLON never throws but must be caught
                        // because of the ByteProcessor interface.
                        throw new IllegalStateException(e);
                    }
                } else {
                    out.add(aName, entry.getValue());
                }
            }
        }
    }

    private static CharSequenceMap<AsciiString> toLowercaseMap(Iterator<? extends CharSequence> valuesIterator) {
        UnsupportedValueConverter<AsciiString> valueConverter = UnsupportedValueConverter.instance();
        CharSequenceMap<AsciiString> result = new CharSequenceMap<>(true, valueConverter, 8);

        while (valuesIterator.hasNext()) {
            AsciiString lowerCased = AsciiString.of(valuesIterator.next()).toLowerCase();
            try {
                int index = lowerCased.forEachByte(FIND_COMMA);
                if (index != -1) {
                    int start = 0;
                    do {
                        result.add(lowerCased.subSequence(start, index, false).trim(), EMPTY_STRING);
                        start = index + 1;
                    } while (start < lowerCased.length() && (index = lowerCased.forEachByte(start, lowerCased.length() - start, FIND_COMMA)) != -1);
                    result.add(lowerCased.subSequence(start, lowerCased.length(), false).trim(), EMPTY_STRING);
                } else {
                    result.add(lowerCased.trim(), EMPTY_STRING);
                }
            } catch (Exception e) {
                // This is not expect to happen because FIND_COMMA never throws but must be caught
                // because of the ByteProcessor interface.
                throw new IllegalStateException(e);
            }
        }
        return result;
    }

    /**
     * Apply HTTP/2 rules while translating status code to {@link HttpResponseStatus}
     *
     * @param status The status from an HTTP/2 frame
     * @return The HTTP/1.x status
     * @throws Http2Exception If there is a problem translating from HTTP/2 to HTTP/1.x
     */
    public static HttpResponseStatus parseStatus(CharSequence status) throws Http2Exception {
        HttpResponseStatus result;
        try {
            result = parseLine(status);
            if (result == HttpResponseStatus.SWITCHING_PROTOCOLS) {
                throw connectionError(PROTOCOL_ERROR, "Invalid HTTP/2 status code '%d'", result.code());
            }
        } catch (Http2Exception e) {
            throw e;
        } catch (Throwable t) {
            throw connectionError(PROTOCOL_ERROR, t, "Unrecognized HTTP status code '%s' encountered in translation to HTTP/1.x", status);
        }
        return result;
    }

    /**
     * Generate an HTTP/2 {code :path} from a URI in accordance with
     * <a href="https://tools.ietf.org/html/rfc7230#section-5.3">rfc7230, 5.3</a>.
     */
    private static AsciiString toHttp2Path(URI uri) {
        StringBuilder pathBuilder = new StringBuilder(length(uri.getRawPath()) + length(uri.getRawQuery()) + length(uri.getRawFragment()) + 2);
        if (!isNullOrEmpty(uri.getRawPath())) {
            pathBuilder.append(uri.getRawPath());
        }
        if (!isNullOrEmpty(uri.getRawQuery())) {
            pathBuilder.append('?');
            pathBuilder.append(uri.getRawQuery());
        }
        if (!isNullOrEmpty(uri.getRawFragment())) {
            pathBuilder.append('#');
            pathBuilder.append(uri.getRawFragment());
        }
        String path = pathBuilder.toString();
        return path.isEmpty() ? EMPTY_REQUEST_PATH : new AsciiString(path);
    }

    private static void setHttp2Authority(String authority, Http2Headers out) {
        // The authority MUST NOT include the deprecated "userinfo" subcomponent
        if (authority != null) {
            if (authority.isEmpty()) {
                out.authority(EMPTY_STRING);
            } else {
                int start = authority.indexOf('@') + 1;
                int length = authority.length() - start;
                if (length == 0) {
                    throw new IllegalArgumentException("authority: " + authority);
                }
                out.authority(new AsciiString(authority, start, length));
            }
        }
    }

    private static final class HTTP2ToHTTPTranslation {

        private static final CharSequenceMap<AsciiString> REQUEST_HEADER_TRANSLATIONS = new CharSequenceMap<>();
        private static final CharSequenceMap<AsciiString> RESPONSE_HEADER_TRANSLATIONS = new CharSequenceMap<>();

        static {
            RESPONSE_HEADER_TRANSLATIONS.add(Http2Headers.PseudoHeaderName.AUTHORITY.value(), HttpHeaderNames.HOST);
            REQUEST_HEADER_TRANSLATIONS.add(RESPONSE_HEADER_TRANSLATIONS);
        }

        static void translateHeaders(Iterable<Map.Entry<CharSequence, CharSequence>> inputHeaders, HttpHeaders output, boolean request) {
            CharSequenceMap<AsciiString> translations;
            if (request) {
                translations = REQUEST_HEADER_TRANSLATIONS;
            } else {
                translations = RESPONSE_HEADER_TRANSLATIONS;
            }

            // lazily created as needed
            StringBuilder cookies = null;

            for (Map.Entry<CharSequence, CharSequence> entry : inputHeaders) {
                final CharSequence name = entry.getKey();
                final CharSequence value = entry.getValue();
                AsciiString translatedName = translations.get(name);
                if (translatedName != null) {
                    output.add(translatedName, AsciiString.of(value));
                } else if (!Http2Headers.PseudoHeaderName.isPseudoHeader(name)) {
                    // https://tools.ietf.org/html/rfc7540#section-8.1.2.3
                    // All headers that start with ':' are only valid in HTTP/2 context
                    if (name.length() == 0 || name.charAt(0) == ':') {
                        throw new IllegalArgumentException("Invalid HTTP/2 header encountered in translation to HTTP/1.1");
                    }
                    if (COOKIE.equals(name)) {
                        // combine the cookie values into 1 header entry.
                        // https://tools.ietf.org/html/rfc7540#section-8.1.2.5
                        if (cookies == null) {
                            cookies = InternalThreadLocalMap.get().stringBuilder();
                        } else if (cookies.length() > 0) {
                            cookies.append("; ");
                        }
                        cookies.append(value);
                    } else {
                        output.add(name, value);
                    }
                }
            }
            if (cookies != null) {
                output.add(COOKIE, cookies.toString());
            }
        }
    }
}
