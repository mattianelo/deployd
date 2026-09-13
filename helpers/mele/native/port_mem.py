"""Apply the reviewed Linux-only restrictions to the pinned MEM loader."""


def port(source):
    changes = [
        ('image_size, pe->opt_hdr->ImageBase);', 'image_size, (unsigned long long)pe->opt_hdr->ImageBase);'),
        ("""    peimage->dl_handle = dlopen(filename, RTLD_NOW);
    if (peimage->dl_handle) {
        return peimage;
    }
""", ""),
        ('open(filename, O_RDWR)', 'open(filename, O_RDONLY | O_CLOEXEC)'),
        ("""    close(fd);

    // patch __alloca_probe""", """    close(fd);
    fd = -1;

    // patch __alloca_probe"""),
    ]
    for before, after in changes:
        if source.count(before) != 1:
            raise ValueError("Pinned MEM source no longer matches the reviewed port")
        source = source.replace(before, after)
    return source
