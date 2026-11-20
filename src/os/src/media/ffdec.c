/* The Linux H.264 and AAC decoders (media/ff.rs): FFmpeg's libavcodec behind a small C API, so the Rust side never
   depends on FFmpeg's struct layouts (they change between FFmpeg versions; this file is compiled against the
   installed headers). H.264 comes out in presentation order, cropped to the stream's display size (libavcodec applies
   the SPS cropping), as planar or semi-planar 8-bit 4:2:0; AAC as interleaved signed 16-bit PCM. */
#include <libavcodec/avcodec.h>
#include <libavutil/channel_layout.h>
#include <libavutil/frame.h>
#include <libavutil/mem.h>
#include <libavutil/pixdesc.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct {
    AVCodecContext *c;
    AVPacket *pkt;
    AVFrame *f;
    int held; /* f holds the picture last handed out */
} NxFf;

typedef struct {
    int w, h;
    /* 0 planar 4:2:0 (y, u, v), 1 semi-planar (y, interleaved uv) */
    int layout;
    const uint8_t *plane[3];
    int stride[3];
    int64_t pts;
    /* AVColorSpace: 1 BT.709, 5 / 6 BT.601, anything else undeclared */
    int matrix;
    int full_range;
} NxFfPicture;

static void say(char *err, int n, const char *what, int e) {
    if (!err || n <= 0) return;
    char b[128] = "";
    if (e) av_strerror(e, b, sizeof b);
    snprintf(err, (size_t)n, "%s%s%s", what, e ? ": " : "", b);
}

/* codec 0 H.264, 1 AAC (extra = the AudioSpecificConfig, rate / channels its stream's) */
void *nxff_open(int codec, const uint8_t *extra, int extra_len, int rate, int channels, char *err, int errlen) {
    const AVCodec *k = avcodec_find_decoder(codec == 0 ? AV_CODEC_ID_H264 : AV_CODEC_ID_AAC);
    if (!k) {
        say(err, errlen, codec == 0 ? "libavcodec has no H.264 decoder" : "libavcodec has no AAC decoder", 0);
        return NULL;
    }
    NxFf *d = av_mallocz(sizeof *d);
    if (!d) return NULL;
    d->c = avcodec_alloc_context3(k);
    d->pkt = av_packet_alloc();
    d->f = av_frame_alloc();
    if (!d->c || !d->pkt || !d->f) {
        say(err, errlen, "out of memory", 0);
        goto fail;
    }
    if (codec == 0) {
        /* slice threads: pictures come out as soon as their access unit allows (frame threads would hold some back) */
        d->c->thread_count = 0;
        d->c->thread_type = FF_THREAD_SLICE;
    } else {
        d->c->sample_rate = rate;
        av_channel_layout_default(&d->c->ch_layout, channels > 0 ? channels : 2);
        d->c->request_sample_fmt = AV_SAMPLE_FMT_S16;
        if (extra && extra_len > 0) {
            d->c->extradata = av_mallocz((size_t)extra_len + AV_INPUT_BUFFER_PADDING_SIZE);
            if (!d->c->extradata) goto fail;
            memcpy(d->c->extradata, extra, (size_t)extra_len);
            d->c->extradata_size = extra_len;
        }
    }
    int e = avcodec_open2(d->c, k, NULL);
    if (e < 0) {
        say(err, errlen, "avcodec_open2", e);
        goto fail;
    }
    return d;
fail:
    avcodec_free_context(&d->c);
    av_packet_free(&d->pkt);
    av_frame_free(&d->f);
    av_free(d);
    return NULL;
}

void nxff_close(void *p) {
    NxFf *d = p;
    if (!d) return;
    avcodec_free_context(&d->c);
    av_packet_free(&d->pkt);
    av_frame_free(&d->f);
    av_free(d);
}

/* One packet (data NULL: end of stream, the decoder lets out what it holds). 0 taken, 1 the decoder is full (take its
   output first, then send again), else an AVERROR. */
int nxff_send(void *p, const uint8_t *data, int len, int64_t pts) {
    NxFf *d = p;
    int e;
    if (!data) {
        e = avcodec_send_packet(d->c, NULL);
        return e == AVERROR_EOF ? 0 : e;
    }
    av_packet_unref(d->pkt);
    if (av_new_packet(d->pkt, len) < 0) return AVERROR(ENOMEM);
    memcpy(d->pkt->data, data, (size_t)len);
    d->pkt->pts = pts;
    d->pkt->dts = AV_NOPTS_VALUE;
    e = avcodec_send_packet(d->c, d->pkt);
    if (e == AVERROR(EAGAIN)) return 1;
    return e;
}

/* The next picture: 1 one in *out (valid until the next call on this decoder), 0 none now, else an AVERROR
   (AVERROR_PATCHWELCOME: a picture format other than 8-bit 4:2:0). */
int nxff_picture(void *p, NxFfPicture *out) {
    NxFf *d = p;
    if (d->held) {
        av_frame_unref(d->f);
        d->held = 0;
    }
    int e = avcodec_receive_frame(d->c, d->f);
    if (e == AVERROR(EAGAIN) || e == AVERROR_EOF) return 0;
    if (e < 0) return e;
    d->held = 1;
    AVFrame *f = d->f;
    memset(out, 0, sizeof *out);
    out->w = f->width;
    out->h = f->height;
    switch (f->format) {
    case AV_PIX_FMT_YUV420P:
    case AV_PIX_FMT_YUVJ420P:
        out->layout = 0;
        for (int i = 0; i < 3; i++) out->plane[i] = f->data[i], out->stride[i] = f->linesize[i];
        break;
    case AV_PIX_FMT_NV12:
        out->layout = 1;
        for (int i = 0; i < 2; i++) out->plane[i] = f->data[i], out->stride[i] = f->linesize[i];
        break;
    default:
        return AVERROR_PATCHWELCOME;
    }
    out->pts = f->pts != AV_NOPTS_VALUE ? f->pts : f->best_effort_timestamp;
    out->matrix = (int)f->colorspace;
    out->full_range = f->color_range == AVCOL_RANGE_JPEG || f->format == AV_PIX_FMT_YUVJ420P;
    return 1;
}

static int16_t s16_of(const AVFrame *f, int ch, int i) {
    int planar = av_sample_fmt_is_planar(f->format);
    int nch = f->ch_layout.nb_channels;
    const uint8_t *b = planar ? f->extended_data[ch] : f->extended_data[0];
    int at = planar ? i : i * nch + ch;
    switch (av_get_packed_sample_fmt(f->format)) {
    case AV_SAMPLE_FMT_S16: return ((const int16_t *)b)[at];
    case AV_SAMPLE_FMT_S32: return (int16_t)(((const int32_t *)b)[at] >> 16);
    case AV_SAMPLE_FMT_FLT: {
        float v = ((const float *)b)[at] * 32768.0f;
        return (int16_t)(v > 32767.0f ? 32767 : v < -32768.0f ? -32768 : v);
    }
    case AV_SAMPLE_FMT_DBL: {
        double v = ((const double *)b)[at] * 32768.0;
        return (int16_t)(v > 32767.0 ? 32767 : v < -32768.0 ? -32768 : v);
    }
    case AV_SAMPLE_FMT_U8: return (int16_t)((((const uint8_t *)b)[at] - 128) << 8);
    default: return 0;
    }
}

/* The next audio frame as interleaved PCM16 in `channels` channels (the stream's mapped onto them: a mono stream
   doubled, extra channels left out): the int16 values written, 0 none now, -2 `cap` too small, else an AVERROR. */
int nxff_audio(void *p, int16_t *out, int cap, int channels) {
    NxFf *d = p;
    int e = avcodec_receive_frame(d->c, d->f);
    if (e == AVERROR(EAGAIN) || e == AVERROR_EOF) return 0;
    if (e < 0) return e;
    int n = d->f->nb_samples, nch = d->f->ch_layout.nb_channels;
    if (channels <= 0) channels = nch;
    if ((long)n * channels > cap) {
        av_frame_unref(d->f);
        return -2;
    }
    for (int i = 0; i < n; i++)
        for (int c = 0; c < channels; c++) out[i * channels + c] = nch <= 0 ? 0 : c < nch ? s16_of(d->f, c, i) : nch == 1 ? s16_of(d->f, 0, i) : 0;
    av_frame_unref(d->f);
    return n * channels;
}

void nxff_flush(void *p) {
    NxFf *d = p;
    if (d->held) {
        av_frame_unref(d->f);
        d->held = 0;
    }
    avcodec_flush_buffers(d->c);
}

const char *nxff_version(void) { return av_version_info(); }

void nxff_error(int e, char *buf, int n) { av_strerror(e, buf, (size_t)n); }
