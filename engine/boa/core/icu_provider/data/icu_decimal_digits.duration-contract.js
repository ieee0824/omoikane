(() => {
    const mappings = [["adlm","𞥐𞥑𞥒𞥓𞥔𞥕𞥖𞥗𞥘𞥙"],["ahom","𑜰𑜱𑜲𑜳𑜴𑜵𑜶𑜷𑜸𑜹"],["arab","٠١٢٣٤٥٦٧٨٩"],["arabext","۰۱۲۳۴۵۶۷۸۹"],["bali","᭐᭑᭒᭓᭔᭕᭖᭗᭘᭙"],["beng","০১২৩৪৫৬৭৮৯"],["bhks","𑱐𑱑𑱒𑱓𑱔𑱕𑱖𑱗𑱘𑱙"],["brah","𑁦𑁧𑁨𑁩𑁪𑁫𑁬𑁭𑁮𑁯"],["cakm","𑄶𑄷𑄸𑄹𑄺𑄻𑄼𑄽𑄾𑄿"],["cham","꩐꩑꩒꩓꩔꩕꩖꩗꩘꩙"],["deva","०१२३४५६७८९"],["diak","𑥐𑥑𑥒𑥓𑥔𑥕𑥖𑥗𑥘𑥙"],["fullwide","０１２３４５６７８９"],["gara","𐵀𐵁𐵂𐵃𐵄𐵅𐵆𐵇𐵈𐵉"],["gong","𑶠𑶡𑶢𑶣𑶤𑶥𑶦𑶧𑶨𑶩"],["gonm","𑵐𑵑𑵒𑵓𑵔𑵕𑵖𑵗𑵘𑵙"],["gujr","૦૧૨૩૪૫૬૭૮૯"],["gukh","𖄰𖄱𖄲𖄳𖄴𖄵𖄶𖄷𖄸𖄹"],["guru","੦੧੨੩੪੫੬੭੮੯"],["hanidec","〇一二三四五六七八九"],["hmng","𖭐𖭑𖭒𖭓𖭔𖭕𖭖𖭗𖭘𖭙"],["hmnp","𞅀𞅁𞅂𞅃𞅄𞅅𞅆𞅇𞅈𞅉"],["java","꧐꧑꧒꧓꧔꧕꧖꧗꧘꧙"],["kali","꤀꤁꤂꤃꤄꤅꤆꤇꤈꤉"],["kawi","𑽐𑽑𑽒𑽓𑽔𑽕𑽖𑽗𑽘𑽙"],["khmr","០១២៣៤៥៦៧៨៩"],["knda","೦೧೨೩೪೫೬೭೮೯"],["krai","𖵰𖵱𖵲𖵳𖵴𖵵𖵶𖵷𖵸𖵹"],["lana","᪀᪁᪂᪃᪄᪅᪆᪇᪈᪉"],["lanatham","᪐᪑᪒᪓᪔᪕᪖᪗᪘᪙"],["laoo","໐໑໒໓໔໕໖໗໘໙"],["latn","0123456789"],["lepc","᱀᱁᱂᱃᱄᱅᱆᱇᱈᱉"],["limb","᥆᥇᥈᥉᥊᥋᥌᥍᥎᥏"],["mathbold","𝟎𝟏𝟐𝟑𝟒𝟓𝟔𝟕𝟖𝟗"],["mathdbl","𝟘𝟙𝟚𝟛𝟜𝟝𝟞𝟟𝟠𝟡"],["mathmono","𝟶𝟷𝟸𝟹𝟺𝟻𝟼𝟽𝟾𝟿"],["mathsanb","𝟬𝟭𝟮𝟯𝟰𝟱𝟲𝟳𝟴𝟵"],["mathsans","𝟢𝟣𝟤𝟥𝟦𝟧𝟨𝟩𝟪𝟫"],["mlym","൦൧൨൩൪൫൬൭൮൯"],["modi","𑙐𑙑𑙒𑙓𑙔𑙕𑙖𑙗𑙘𑙙"],["mong","᠐᠑᠒᠓᠔᠕᠖᠗᠘᠙"],["mroo","𖩠𖩡𖩢𖩣𖩤𖩥𖩦𖩧𖩨𖩩"],["mtei","꯰꯱꯲꯳꯴꯵꯶꯷꯸꯹"],["mymr","၀၁၂၃၄၅၆၇၈၉"],["mymrepka","𑛚𑛛𑛜𑛝𑛞𑛟𑛠𑛡𑛢𑛣"],["mymrpao","𑛐𑛑𑛒𑛓𑛔𑛕𑛖𑛗𑛘𑛙"],["mymrshan","႐႑႒႓႔႕႖႗႘႙"],["mymrtlng","꧰꧱꧲꧳꧴꧵꧶꧷꧸꧹"],["nagm","𞓰𞓱𞓲𞓳𞓴𞓵𞓶𞓷𞓸𞓹"],["newa","𑑐𑑑𑑒𑑓𑑔𑑕𑑖𑑗𑑘𑑙"],["nkoo","߀߁߂߃߄߅߆߇߈߉"],["olck","᱐᱑᱒᱓᱔᱕᱖᱗᱘᱙"],["onao","𞗱𞗲𞗳𞗴𞗵𞗶𞗷𞗸𞗹𞗺"],["orya","୦୧୨୩୪୫୬୭୮୯"],["osma","𐒠𐒡𐒢𐒣𐒤𐒥𐒦𐒧𐒨𐒩"],["outlined","𜳰𜳱𜳲𜳳𜳴𜳵𜳶𜳷𜳸𜳹"],["rohg","𐴰𐴱𐴲𐴳𐴴𐴵𐴶𐴷𐴸𐴹"],["saur","꣐꣑꣒꣓꣔꣕꣖꣗꣘꣙"],["segment","🯰🯱🯲🯳🯴🯵🯶🯷🯸🯹"],["shrd","𑇐𑇑𑇒𑇓𑇔𑇕𑇖𑇗𑇘𑇙"],["sind","𑋰𑋱𑋲𑋳𑋴𑋵𑋶𑋷𑋸𑋹"],["sinh","෦෧෨෩෪෫෬෭෮෯"],["sora","𑃰𑃱𑃲𑃳𑃴𑃵𑃶𑃷𑃸𑃹"],["sund","᮰᮱᮲᮳᮴᮵᮶᮷᮸᮹"],["sunu","𑯰𑯱𑯲𑯳𑯴𑯵𑯶𑯷𑯸𑯹"],["takr","𑛀𑛁𑛂𑛃𑛄𑛅𑛆𑛇𑛈𑛉"],["talu","᧐᧑᧒᧓᧔᧕᧖᧗᧘᧙"],["tamldec","௦௧௨௩௪௫௬௭௮௯"],["telu","౦౧౨౩౪౫౬౭౮౯"],["thai","๐๑๒๓๔๕๖๗๘๙"],["tibt","༠༡༢༣༤༥༦༧༨༩"],["tirh","𑓐𑓑𑓒𑓓𑓔𑓕𑓖𑓗𑓘𑓙"],["tnsa","𖫀𖫁𖫂𖫃𖫄𖫅𖫆𖫇𖫈𖫉"],["vaii","꘠꘡꘢꘣꘤꘥꘦꘧꘨꘩"],["wara","𑣠𑣡𑣢𑣣𑣤𑣥𑣦𑣧𑣨𑣩"],["wcho","𞋰𞋱𞋲𞋳𞋴𞋵𞋶𞋷𞋸𞋹"]];
    function check(condition, message) {
        if (!condition) throw new Error(message);
    }
    for (const [nu, raw] of mappings) {
        const digits = [...raw];
        const formatter = new Intl.DurationFormat('en-US', {
            numberingSystem: nu, style: 'digital', fractionalDigits: 1
        });
        check(formatter.resolvedOptions().numberingSystem === nu, `${nu}: duration resolution`);
        const record = {hours: 12, minutes: 34, seconds: 56, milliseconds: 700};
        const parts = formatter.formatToParts(record);
        for (const [unit, first, second] of [['hour', 1, 2], ['minute', 3, 4], ['second', 5, 6]]) {
            const value = parts.filter(p => p.unit === unit && p.type === 'integer').map(p => p.value).join('');
            check(value === digits[first] + digits[second], `${nu}: ${unit} digits`);
        }
        check(parts.filter(p => p.type === 'fraction').map(p => p.value).join('') === digits[7], `${nu}: duration fractional digits`);
        check(parts.map(p => p.value).join('') === formatter.format(record), `${nu}: duration parts join`);
    }
    const unknown = new Intl.DurationFormat('en-US', {numberingSystem: 'foobar', style: 'digital'});
    check(unknown.resolvedOptions().numberingSystem === 'latn' && unknown.format({hours: 12, minutes: 34}) === '12:34:00', 'unknown duration nu fallback');
    return mappings.length === 77;
})()
